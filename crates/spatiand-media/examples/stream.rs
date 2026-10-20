//! A film played for a while with nobody watching, to see how it is fetched: how often the
//! picture stood still for want of the film, and for how long.
//!
//! ```text
//! cargo run -p spatiand-media --example stream -- <url or file> [seconds] [seek to, ten seconds in]
//! ```

use std::time::{Duration, Instant};

use spatiand_media::player::Player;

fn main() -> Result<(), String> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    std::env::set_var("SPATIAND_MEDIA_MUTE", "1");
    let mut args = std::env::args().skip(1);
    let film = args.next().ok_or("which film?")?;
    let seconds: f64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(30.0);
    let mut jump: Option<f64> = args.next().and_then(|s| s.parse().ok());
    let opened = Instant::now();
    let mut player = Player::open(&film)?;
    println!("opened in {:.2}s", opened.elapsed().as_secs_f64());
    let started = Instant::now();
    let (mut first, mut last_new, mut last_time) = (None, Instant::now(), -1.0);
    let (mut stalls, mut stalled) = (0, 0.0);
    while started.elapsed().as_secs_f64() < seconds && !player.ended() {
        if started.elapsed().as_secs_f64() > 10.0 {
            if let Some(to) = jump.take() {
                println!("  seeking to {to}s");
                player.seek(to);
                last_new = Instant::now();
            }
        }
        if let (Some(picture), _) = player.picture() {
            if picture.time != last_time {
                first.get_or_insert_with(|| started.elapsed().as_secs_f64());
                let gap = last_new.elapsed().as_secs_f64();
                // A picture late by more than a quarter second is a stall the eye sees.
                if first.is_some() && last_time >= 0.0 && gap > 0.25 {
                    stalls += 1;
                    stalled += gap;
                    println!("  stood still {gap:.2}s at {:.1}s of the film", picture.time);
                }
                last_time = picture.time;
                last_new = Instant::now();
            }
        }
        std::thread::sleep(Duration::from_millis(4));
    }
    println!("first picture after {:.2}s; {stalls} stalls, {stalled:.1}s standing still; at {:.1}s of the film after {:.1}s", first.unwrap_or(-1.0), player.position(), started.elapsed().as_secs_f64());
    Ok(())
}
