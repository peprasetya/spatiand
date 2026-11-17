//! An application that draws its own two eye views from the room's poses: how the pose
//! channel is used, and how it is checked.
//!
//! Twenty-four posts stand in a ring two metres round the wearer, on a floor of lines: they are the
//! room's, drawn from each eye's pose, and stay where they are as the head turns. A cross
//! floats a metre in front of the eyes: it is the head's, and stays in the middle of the
//! picture. Each picture is handed over with the pose it was drawn from
//! (`App::set_frame_pose`), so that the room can turn it by however far the head has moved
//! since. Where the room has no poses to give (no glasses, a desktop) it is a flat window
//! looking at the same ring from the middle of it.
//!
//! Against a room with no display and a head that is not there (the Deck's
//! `SPATIAND_BACKEND=headless SPATIAND_HEADLESS_HEAD=<degrees a second>`):
//!
//! ```text
//! POSES_CHECK=3 POSES_PNG=/tmp/poses.png WAYLAND_DISPLAY=<the room's> cargo run -p spatiand-app --example poses
//! ```
//!
//! draws for three seconds, then says what it saw -- how many poses, how far the head turned,
//! the eyes -- and asks the room's control socket which head it was told the last picture was
//! drawn for: the number and the quaternion must be the ones that picture's pose carried. It
//! ends with 0 if all of that held and 1 if not. `POSES_EXPECT=none` is the same check of a
//! room that has no poses: it must say so, and the window must draw flat.

use std::time::{Duration, Instant};

use spatiand_app::glow::{self, HasContext};
use spatiand_app::pose::{multiply, rotate, Eye};
use spatiand_app::{App, Event, EyeLayout, HeadPose, Layer, Poses};

/// One eye's share of the picture.
const EYE: (u32, u32) = (960, 540);

fn main() -> Result<(), String> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let check: Option<f64> = std::env::var("POSES_CHECK").ok().and_then(|s| s.parse().ok());
    let expect_none = std::env::var("POSES_EXPECT").as_deref() == Ok("none");
    let png = std::env::var("POSES_PNG").ok();

    let mut app = App::connect("id.prasetya.spatiand.poses")?;
    let window = app.open("Poses", EYE);
    app.ask_poses();

    let mut scene: Option<Scene> = None;
    let mut seen = Seen::default();
    let mut stereo = false;
    let started = Instant::now();
    let mut last: Option<HeadPose> = None;

    loop {
        let mut draw = false;
        for event in app.wait(Some(Duration::from_millis(100)))? {
            match event {
                Event::CloseRequested { .. } | Event::Quit { .. } => return Ok(()),
                Event::Ready { .. } | Event::Frame { .. } => draw = true,
                Event::Poses { available } => {
                    // Two eyes of the application's own while there is a head to draw them
                    // from; a flat window while there is not.
                    stereo = available;
                    match app.poses() {
                        Poses::Unavailable(why) => log::info!("no poses: {why}"),
                        _ => log::info!("the room's poses are here"),
                    }
                    if stereo {
                        app.set_eyes(window, EyeLayout::SideBySide, false);
                        app.set_layer(window, Layer::Projection);
                        app.resize(window, (EYE.0 * 2, EYE.1));
                    } else {
                        app.set_eyes(window, EyeLayout::Mono, false);
                        app.set_layer(window, Layer::Window);
                        app.set_frame_pose(window, None);
                        app.resize(window, EYE);
                    }
                    seen.answers.push(available);
                    draw = true;
                }
                Event::EyeSize { width, height } => log::info!("the viewer wants each eye {width} x {height}"),
                Event::LayerRefused { layer, .. } => log::warn!("{layer:?} was refused: drawn as a window"),
                _ => {}
            }
        }
        let over = check.is_some_and(|seconds| started.elapsed().as_secs_f64() >= seconds);
        if draw || over {
            // As late as can be: nothing is between this read and the drawing.
            let pose = if stereo { app.pose() } else { None };
            if let Some(pose) = &pose {
                app.set_frame_pose(window, Some(pose));
            }
            // Asked before drawing: it goes with the picture, and comes back when the room
            // wants the next.
            if !over {
                app.ask_frame(window);
            }
            let eyes: Vec<Eye> = match &pose {
                Some(pose) => pose.eyes.to_vec(),
                // Flat, or the moment before the room's first pose: one eye, in the middle.
                None => vec![Eye { orientation: [0.0, 0.0, 0.0, 1.0], position: [0.0; 3], fov: [-0.36, 0.36, 0.21, -0.21] }],
            };
            // The head's own things hang from the point between the eyes, not from the pose's
            // position, which is the neck the head turns on.
            let head = pose.map_or(([0.0, 0.0, 0.0, 1.0], [0.0; 3]), |p| {
                (p.orientation, [0, 1, 2].map(|i| (p.eyes[0].position[i] + p.eyes[1].position[i]) / 2.0))
            });
            let save = png.as_deref().filter(|_| over || check.is_none());
            let drawn = app.draw(window, |gl, size| {
                let scene = scene.get_or_insert_with(|| Scene::new(gl.api()).expect("the scene's shader"));
                scene.draw(gl.api(), size, &eyes, head);
                if let Some(path) = save {
                    if let Err(e) = write_png(gl.api(), size, path) {
                        log::warn!("{path}: {e}");
                    }
                }
            })?;
            if drawn {
                seen.drawn += 1;
                if let Some(pose) = &pose {
                    seen.note(pose);
                }
                last = pose;
            }
        }
        if over {
            // Let the last picture reach the room before asking it what came with it.
            for _ in 0..5 {
                app.wait(Some(Duration::from_millis(50)))?;
            }
            let ok = if expect_none { seen.report_none(&app) } else { seen.report(started.elapsed(), last.as_ref()) };
            std::process::exit(if ok { 0 } else { 1 });
        }
    }
}

/// What the room's poses were, over a run.
#[derive(Default)]
struct Seen {
    answers: Vec<bool>,
    drawn: u32,
    poses: u32,
    /// Poses that were not the one before: the room writes at its own rate.
    fresh: u32,
    first: Option<HeadPose>,
    newest: Option<HeadPose>,
    tokens_in_order: bool,
    whole: bool,
}

impl Seen {
    fn note(&mut self, pose: &HeadPose) {
        if self.first.is_none() {
            self.first = Some(*pose);
            self.tokens_in_order = true;
            self.whole = true;
        }
        self.poses += 1;
        if let Some(before) = &self.newest {
            if before.sample_ns != pose.sample_ns {
                self.fresh += 1;
            }
            self.tokens_in_order &= pose.sample_ns >= before.sample_ns && pose.token >= before.token;
        }
        // A pose is a unit quaternion, eyes a few centimetres apart with the left on the left
        // (as the head sees it), looking where the head looks, through a view that opens
        // both ways: half of one, or the wrong bytes, is none of these.
        let length = pose.orientation.iter().map(|c| c * c).sum::<f32>().sqrt();
        let inverse = [-pose.orientation[0], -pose.orientation[1], -pose.orientation[2], pose.orientation[3]];
        let apart = rotate(inverse, [0, 1, 2].map(|i| pose.eyes[1].position[i] - pose.eyes[0].position[i]));
        let same = |q: [f32; 4]| q.iter().zip(pose.orientation).map(|(a, b)| a * b).sum::<f32>().abs() > 0.9999;
        let views = pose.eyes.iter().all(|e| e.fov[0] < 0.0 && e.fov[1] > 0.0 && e.fov[2] > 0.0 && e.fov[3] < 0.0 && same(e.orientation));
        self.whole &= (length - 1.0).abs() < 1e-3 && (0.04..0.09).contains(&apart[0]) && apart[1].abs() < 1e-3 && apart[2].abs() < 1e-3 && views;
        self.whole &= pose.predicted_ns >= pose.sample_ns && (spatiand_app::pose::now_ns() - pose.sample_ns).abs() < 1_000_000_000;
        self.newest = Some(*pose);
    }

    /// Say what was seen, and ask the room what it was told of the last picture.
    fn report(&self, over: Duration, last: Option<&HeadPose>) -> bool {
        let (Some(first), Some(newest), Some(last)) = (&self.first, &self.newest, last) else {
            println!("FAILED: no pose was ever read (the room answered {:?})", self.answers);
            return false;
        };
        let dot: f32 = first.orientation.iter().zip(newest.orientation).map(|(a, b)| a * b).sum();
        let turned = (2.0 * dot.abs().min(1.0).acos()).to_degrees();
        println!("{} pictures drawn in {:.1} s from {} poses read, {} of them new", self.drawn, over.as_secs_f64(), self.poses, self.fresh);
        println!("the head turned {turned:.1} degrees; numbers {} to {}", first.token, newest.token);
        println!(
            "eyes {:.1} mm apart; the left sees {:.1} to {:.1} degrees across, {:.1} to {:.1} up and down",
            ((0..3).map(|i| (newest.eyes[1].position[i] - newest.eyes[0].position[i]).powi(2)).sum::<f32>()).sqrt() * 1000.0,
            newest.eyes[0].fov[0].to_degrees(),
            newest.eyes[0].fov[1].to_degrees(),
            newest.eyes[0].fov[3].to_degrees(),
            newest.eyes[0].fov[2].to_degrees(),
        );
        let mut ok = true;
        let mut must = |held: bool, what: &str| {
            println!("{} {what}", if held { "ok:    " } else { "FAILED:" });
            ok &= held;
        };
        must(self.answers == [true], "the room handed over its poses, once");
        must(self.fresh >= 10, "poses went on arriving");
        must(self.whole, "every pose read was a whole one: a unit quaternion, eyes apart and level, views that open, times of now");
        must(self.tokens_in_order, "times and numbers never went backwards");
        must(turned > 1.0, "the head turned");
        match asked_of_the_room() {
            Ok(Some((token, q))) => {
                let sent = last.orientation.map(|c| (c * 1e6).round() as i32);
                println!("the last picture was drawn from number {} {sent:?}; the room was told {token} {q:?}", last.token);
                must(token == last.token && token != 0, "the pose's number came back with the picture");
                must(sent.iter().zip(q).all(|(a, b)| (a - b).abs() <= 2), "and so did its orientation");
            }
            Ok(None) => must(false, "the room was told which head the picture was drawn for"),
            Err(e) => must(false, &format!("the room's control socket answered ({e})")),
        }
        ok
    }

    /// The same for a room with no poses: it must have said so, and the window drawn anyway.
    fn report_none(&self, app: &App) -> bool {
        println!("the room answered {:?}; it says {:?}; {} pictures drawn", self.answers, app.poses(), self.drawn);
        let ok = self.answers == [false] && matches!(app.poses(), Poses::Unavailable(_)) && app.pose().is_none() && self.poses == 0 && self.drawn > 0;
        println!("{} no poses to give, said once, and a flat window drawn", if ok { "ok:    " } else { "FAILED:" });
        ok
    }
}

/// Ask the room's control socket (`crates/spatiand/src/control.rs`) for the head this
/// application's window said its picture was drawn for: the number and the quaternion, in
/// millionths.
fn asked_of_the_room() -> Result<Option<(u32, [i32; 4])>, String> {
    use std::io::{BufRead, BufReader, Write};
    let dir = std::env::var("XDG_RUNTIME_DIR").map_err(|_| "no XDG_RUNTIME_DIR")?;
    let stream = std::os::unix::net::UnixStream::connect(format!("{dir}/spatiand/control")).map_err(|e| e.to_string())?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).map_err(|e| e.to_string())?;
    let mut lines = BufReader::new(stream.try_clone().map_err(|e| e.to_string())?).lines();
    let ask = |line: &str| writeln!(&stream, "{line}").map_err(|e| e.to_string());
    ask("list")?;
    let mut window = None;
    for line in lines.by_ref() {
        let line = line.map_err(|e| e.to_string())?;
        let words: Vec<&str> = line.split_whitespace().collect();
        match words.as_slice() {
            ["window", id, app, ..] if app.starts_with("id.prasetya.spatiand.poses") => window = Some(id.to_string()),
            ["end"] => break,
            _ => {}
        }
    }
    let window = window.ok_or("the room lists no window of this application's")?;
    ask(&format!("frame {window}"))?;
    let line = lines.next().ok_or("no answer")?.map_err(|e| e.to_string())?;
    let words: Vec<&str> = line.split_whitespace().collect();
    match words.as_slice() {
        ["frame", "none"] => Ok(None),
        ["frame", token, x, y, z, w] => {
            let number = |s: &str| s.parse::<i32>().map_err(|_| format!("{line:?}"));
            Ok(Some((token.parse().map_err(|_| format!("{line:?}"))?, [number(x)?, number(y)?, number(z)?, number(w)?])))
        }
        _ => Err(line),
    }
}

/// The ring of posts, the floor and the cross, and the one shader they are drawn with.
struct Scene {
    program: glow::Program,
    mvp: glow::UniformLocation,
    colour: glow::UniformLocation,
    vao: glow::VertexArray,
    /// Where each shape's vertices begin in the one buffer, and how many it has.
    cube: (i32, i32),
    floor: (i32, i32),
    cross: (i32, i32),
}

impl Scene {
    fn new(g: &glow::Context) -> Result<Scene, String> {
        const VERTEX: &str = "#version 300 es\nuniform mat4 u_mvp;\nlayout(location = 0) in vec3 a_at;\nvoid main() { gl_Position = u_mvp * vec4(a_at, 1.0); }\n";
        const FRAGMENT: &str = "#version 300 es\nprecision mediump float;\nuniform vec4 u_colour;\nout vec4 colour;\nvoid main() { colour = u_colour; }\n";
        // A cube a unit across, about its middle: twelve triangles.
        let corner = |i: usize| [(i & 1) as f32 - 0.5, ((i >> 1) & 1) as f32 - 0.5, ((i >> 2) & 1) as f32 - 0.5];
        let faces: [[usize; 4]; 6] = [[0, 1, 3, 2], [4, 6, 7, 5], [0, 4, 5, 1], [2, 3, 7, 6], [0, 2, 6, 4], [1, 5, 7, 3]];
        let mut vertices: Vec<f32> = Vec::new();
        for face in faces {
            for i in [0, 1, 2, 0, 2, 3] {
                vertices.extend(corner(face[i]));
            }
        }
        let cube = (0, 36);
        // A floor of lines a metre apart, ten metres each way, a metre and a half down.
        for n in -10..=10 {
            let n = n as f32;
            vertices.extend([n, -1.5, -10.0, n, -1.5, 10.0, -10.0, -1.5, n, 10.0, -1.5, n]);
        }
        let floor = (36, 84);
        // A cross in the plane across the view.
        vertices.extend([-0.06, 0.0, 0.0, 0.06, 0.0, 0.0, 0.0, -0.06, 0.0, 0.0, 0.06, 0.0]);
        let cross = (120, 4);
        unsafe {
            let program = g.create_program()?;
            for (kind, source) in [(glow::VERTEX_SHADER, VERTEX), (glow::FRAGMENT_SHADER, FRAGMENT)] {
                let shader = g.create_shader(kind)?;
                g.shader_source(shader, source);
                g.compile_shader(shader);
                if !g.get_shader_compile_status(shader) {
                    return Err(g.get_shader_info_log(shader));
                }
                g.attach_shader(program, shader);
            }
            g.link_program(program);
            if !g.get_program_link_status(program) {
                return Err(g.get_program_info_log(program));
            }
            let vao = g.create_vertex_array()?;
            g.bind_vertex_array(Some(vao));
            let buffer = g.create_buffer()?;
            g.bind_buffer(glow::ARRAY_BUFFER, Some(buffer));
            let bytes = std::slice::from_raw_parts(vertices.as_ptr() as *const u8, vertices.len() * 4);
            g.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes, glow::STATIC_DRAW);
            g.enable_vertex_attrib_array(0);
            g.vertex_attrib_pointer_f32(0, 3, glow::FLOAT, false, 0, 0);
            g.bind_vertex_array(None);
            Ok(Scene {
                program,
                mvp: g.get_uniform_location(program, "u_mvp").ok_or("no u_mvp")?,
                colour: g.get_uniform_location(program, "u_colour").ok_or("no u_colour")?,
                vao,
                cube,
                floor,
                cross,
            })
        }
    }

    /// Each eye's view into its share of the picture, the left eye's first.
    fn draw(&self, g: &glow::Context, size: (u32, u32), eyes: &[Eye], head: ([f32; 4], [f32; 3])) {
        let width = size.0 as i32 / eyes.len() as i32;
        unsafe {
            g.disable(glow::DEPTH_TEST);
            g.disable(glow::BLEND);
            g.disable(glow::CULL_FACE);
            g.disable(glow::SCISSOR_TEST);
            g.clear_color(0.02, 0.03, 0.06, 1.0);
            g.clear(glow::COLOR_BUFFER_BIT);
            g.use_program(Some(self.program));
            g.bind_vertex_array(Some(self.vao));
            for (n, eye) in eyes.iter().enumerate() {
                g.viewport(n as i32 * width, 0, width, size.1 as i32);
                let from_world = multiply(&eye.projection(0.05, 100.0), &eye.view());
                let shape = |model: &[f32; 16], colour: [f32; 4], mode: u32, (first, count): (i32, i32)| {
                    g.uniform_matrix_4_f32_slice(Some(&self.mvp), false, &multiply(&from_world, model));
                    g.uniform_4_f32_slice(Some(&self.colour), &colour);
                    g.draw_arrays(mode, first, count);
                };
                // The room's: the floor, then the posts, the farthest from this eye first --
                // there is no depth here to sort them.
                shape(&placed([0.0, 0.0, 0.0, 1.0], [0.0; 3], [1.0; 3]), [0.2, 0.3, 0.4, 1.0], glow::LINES, self.floor);
                let mut posts: Vec<(usize, [f32; 3])> = (0..24)
                    .map(|i| {
                        let angle = i as f32 * std::f32::consts::TAU / 24.0;
                        // The first straight ahead (-Z), the rest going round to the right.
                        (i, [2.0 * angle.sin(), -0.1, -2.0 * angle.cos()])
                    })
                    .collect();
                let far = |p: &[f32; 3]| (0..3).map(|i| (p[i] - eye.position[i]).powi(2)).sum::<f32>();
                posts.sort_by(|a, b| far(&b.1).total_cmp(&far(&a.1)));
                for (i, at) in posts {
                    let hue = i as f32 / 24.0 * 6.0;
                    let channel = |shift: f32| (((hue + shift) % 6.0 - 3.0).abs() - 1.0).clamp(0.0, 1.0);
                    shape(&placed([0.0, 0.0, 0.0, 1.0], at, [0.12, 0.5, 0.12]), [channel(0.0), channel(4.0), channel(2.0), 1.0], glow::TRIANGLES, self.cube);
                }
                // The head's: a metre in front of the eyes, wherever they are turned.
                let ahead = rotate(head.0, [0.0, 0.0, -1.0]);
                let at = [head.1[0] + ahead[0], head.1[1] + ahead[1], head.1[2] + ahead[2]];
                shape(&placed(head.0, at, [1.0; 3]), [1.0, 1.0, 1.0, 1.0], glow::LINES, self.cross);
            }
            g.bind_vertex_array(None);
            g.use_program(None);
        }
    }
}

/// A shape turned by `q`, stretched by `scale` and put at `at`: its model matrix, column-major.
fn placed(q: [f32; 4], at: [f32; 3], scale: [f32; 3]) -> [f32; 16] {
    let x = rotate(q, [scale[0], 0.0, 0.0]);
    let y = rotate(q, [0.0, scale[1], 0.0]);
    let z = rotate(q, [0.0, 0.0, scale[2]]);
    [x[0], x[1], x[2], 0.0, y[0], y[1], y[2], 0.0, z[0], z[1], z[2], 0.0, at[0], at[1], at[2], 1.0]
}

/// What was drawn, as a file: row 0 of the picture is its top, so the rows are in order.
fn write_png(g: &glow::Context, size: (u32, u32), path: &str) -> Result<(), String> {
    let mut pixels = vec![0u8; (size.0 * size.1 * 4) as usize];
    unsafe {
        g.read_pixels(0, 0, size.0 as i32, size.1 as i32, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelPackData::Slice(Some(&mut pixels)));
    }
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), size.0, size.1);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(&pixels).map_err(|e| e.to_string())
}
