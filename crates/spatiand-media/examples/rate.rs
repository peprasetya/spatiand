//! A film played for a while as SpatiReel plays it -- each new picture drawn into a window-sized
//! picture, which is read back as a window's is where it cannot be handed over -- with nobody
//! watching and no room: how many pictures a second were shown, how many were skipped for being
//! late, and how much of the processor it took.
//!
//! ```text
//! cargo run -p spatiand-media --example rate -- <film> [seconds] [noread]
//! ```

use std::time::{Duration, Instant};

use spatiand_app::glow::{self, HasContext};
use spatiand_app::Gl;
use spatiand_media::player::Player;
use spatiand_media::screen::Screen;

/// This process's processor time so far, in seconds (Linux and Android).
fn cpu_seconds() -> f64 {
    let stat = std::fs::read_to_string("/proc/self/stat").unwrap_or_default();
    // After the program's name, in brackets: the 14th and 15th fields are user and system ticks.
    let rest = stat.rsplit_once(')').map_or("", |(_, rest)| rest);
    let fields: Vec<&str> = rest.split_whitespace().collect();
    let ticks = |i: usize| fields.get(i).and_then(|f| f.parse::<f64>().ok()).unwrap_or(0.0);
    (ticks(11) + ticks(12)) / 100.0
}

fn main() -> Result<(), String> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    std::env::set_var("SPATIAND_MEDIA_MUTE", "1");
    let mut args = std::env::args().skip(1);
    let film = args.next().ok_or("which film?")?;
    let seconds: f64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(20.0);
    let read_back = args.next().as_deref() != Some("noread");
    let gl = Gl::new()?;
    let mut player = Player::open(&film)?;
    let mut screen = Screen::new(&gl)?;
    let info = player.info().clone();
    let rate = if info.frame_rate > 0.0 { info.frame_rate } else { 30.0 };
    // The window: the film's size, no more than one eye of the glasses.
    let scale = (1920.0 / info.width.max(1) as f64).min(1080.0 / info.height.max(1) as f64).min(1.0);
    let (w, h) = (((info.width as f64 * scale) as u32).max(1), ((info.height as f64 * scale) as u32).max(1));
    let g = gl.api();
    let target = unsafe {
        let texture = g.create_texture()?;
        g.bind_texture(glow::TEXTURE_2D, Some(texture));
        g.tex_image_2d(glow::TEXTURE_2D, 0, glow::RGBA8 as i32, w as i32, h as i32, 0, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelUnpackData::Slice(None));
        let target = g.create_framebuffer()?;
        g.bind_framebuffer(glow::FRAMEBUFFER, Some(target));
        g.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(texture), 0);
        target
    };
    let mut pixels = vec![0u8; (w * h * 4) as usize];
    // From the first picture on: opening and the first decode are not the rate.
    let (mut shown, mut skipped, mut last_time) = (0u64, 0u64, f64::NAN);
    let (mut began, mut cpu_began) = (None, 0.0);
    let mut kind = None;
    let mut worst = Duration::ZERO;
    let mut last_new = Instant::now();
    let opened = Instant::now();
    while !player.ended() {
        if let (Some(picture), true) = player.picture() {
            if began.is_none() {
                began = Some(Instant::now());
                cpu_began = cpu_seconds();
                kind = Some(picture.pixels);
            } else {
                shown += 1;
                worst = worst.max(last_new.elapsed());
                let frames = ((picture.time - last_time) * rate).round() as i64;
                skipped += (frames - 1).max(0) as u64;
            }
            last_new = Instant::now();
            last_time = picture.time;
            screen.load(&gl, &picture);
            unsafe {
                g.bind_framebuffer(glow::FRAMEBUFFER, Some(target));
                g.viewport(0, 0, w as i32, h as i32);
                screen.draw(&gl, (w, h), [0.0, 0.0, w as f32, h as f32], [0.0, 0.0, 1.0, 1.0]);
                if read_back {
                    g.read_pixels(0, 0, w as i32, h as i32, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelPackData::Slice(Some(&mut pixels)));
                } else {
                    g.finish();
                }
            }
        }
        if began.is_some_and(|b: Instant| b.elapsed().as_secs_f64() >= seconds) || opened.elapsed().as_secs_f64() > seconds + 30.0 {
            break;
        }
        // The glasses' pace: SpatiReel draws when the room asks, sixty times a second or more.
        std::thread::sleep(Duration::from_millis(4));
    }
    let Some(began) = began else { return Err("no picture came".into()) };
    let elapsed = began.elapsed().as_secs_f64();
    let cpu = cpu_seconds() - cpu_began;
    println!(
        "{}x{} {} at {:.2}/s, {}: {:?} pictures, drawn into {w}x{h}{}",
        info.width,
        info.height,
        info.video_codec,
        rate,
        info.hardware.unwrap_or("decoded in software"),
        kind,
        if read_back { " and read back" } else { "" },
    );
    println!(
        "{:.1} s: {shown} shown ({:.1} a second), {skipped} skipped, longest wait {:.0} ms; processor {:.0}% of one core",
        elapsed,
        shown as f64 / elapsed,
        worst.as_secs_f64() * 1000.0,
        cpu / elapsed * 100.0,
    );
    Ok(())
}
