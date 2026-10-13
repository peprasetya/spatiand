//! One frame of a film, decoded as SpatiReel would decode it and drawn as it would draw it,
//! to a PNG -- with no room to connect to. How the engine is checked on a machine that has a
//! GPU and no Spatiand: which decoder was used, and whether its picture survived the trip.
//!
//! ```text
//! cargo run -p spatiand-media --example frame -- film.mkv out.png [seconds]
//! ```

use spatiand_app::glow::{self, HasContext};
use spatiand_app::Gl;
use spatiand_media::player::Player;
use spatiand_media::screen::Screen;

fn main() -> Result<(), String> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    std::env::set_var("SPATIAND_MEDIA_MUTE", "1");
    let mut args = std::env::args().skip(1);
    let (film, out) = (args.next().ok_or("which film?")?, args.next().ok_or("to which file?")?);
    let at: f64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(1.0);
    let gl = Gl::new()?;
    let mut player = Player::open(&film)?;
    player.seek(at);
    let mut screen = Screen::new(&gl)?;
    let started = std::time::Instant::now();
    let picture = loop {
        if let (Some(picture), true) = player.picture() {
            if picture.time >= at - 0.05 {
                break picture;
            }
        }
        if started.elapsed().as_secs() > 15 {
            return Err("no picture came".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    println!("{}x{} at {:.2}s, {:?}, {:?}, decoder: {}", picture.width, picture.height, picture.time, picture.pixels, picture.colour, player.info().hardware.unwrap_or("software"));
    let (w, h) = (picture.width, picture.height);
    let g = gl.api();
    let mut pixels = vec![0u8; (w * h * 4) as usize];
    unsafe {
        let texture = g.create_texture()?;
        g.bind_texture(glow::TEXTURE_2D, Some(texture));
        g.tex_image_2d(glow::TEXTURE_2D, 0, glow::RGBA8 as i32, w as i32, h as i32, 0, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelUnpackData::Slice(None));
        let target = g.create_framebuffer()?;
        g.bind_framebuffer(glow::FRAMEBUFFER, Some(target));
        g.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(texture), 0);
        g.viewport(0, 0, w as i32, h as i32);
        screen.load(&gl, &picture);
        g.bind_framebuffer(glow::FRAMEBUFFER, Some(target));
        screen.draw(&gl, (w, h), [0.0, 0.0, w as f32, h as f32], [0.0, 0.0, 1.0, 1.0]);
        g.read_pixels(0, 0, w as i32, h as i32, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelPackData::Slice(Some(&mut pixels)));
    }
    let file = std::fs::File::create(&out).map_err(|e| e.to_string())?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header().and_then(|mut writer| writer.write_image_data(&pixels)).map_err(|e| e.to_string())?;
    // How even the picture is not, as a number: a picture that is all one colour did not
    // survive, whatever colour that is.
    let mean = pixels.chunks_exact(4).map(|p| p[0] as f64).sum::<f64>() / (w * h) as f64;
    let spread = (pixels.chunks_exact(4).map(|p| (p[0] as f64 - mean).powi(2)).sum::<f64>() / (w * h) as f64).sqrt();
    println!("wrote {out}: red is {mean:.0} on average and varies by {spread:.0}");
    Ok(())
}
