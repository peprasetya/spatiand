//! A film moved about in with nobody watching: for each place asked for, where the first
//! picture after the move was, how long it took to come, and what the film's length was taken
//! to be then. How seeking in a file with no times (`timing`) is checked.
//!
//! ```text
//! cargo run -p spatiand-media --example seeks -- <film> <play for>:<then go to> ...
//! ```
//!
//! `seeks long.h264 3:40 2:10 2:55` plays three seconds, goes to 40 s, plays two, goes to 10 s,
//! plays two, goes to 55 s, and then plays to the end. A first step of `0:<to>` moves before
//! anything has been shown, as a film taken up where it was left is.

use std::time::{Duration, Instant};

use spatiand_media::player::Player;

fn main() -> Result<(), String> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    std::env::set_var("SPATIAND_MEDIA_MUTE", "1");
    let mut args = std::env::args().skip(1);
    let film = args.next().ok_or("which film?")?;
    let steps: Vec<(f64, f64)> = args.filter_map(|step| step.split_once(':').and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?)))).collect();
    let mut player = Player::open(&film)?;
    let length = |player: &Player| format!("{:.2} s{}", player.info().duration, if player.info().duration_guessed { " (a guess)" } else { "" });
    // Plays for `seconds`, or to the end: how many pictures came and the last one's time.
    let play = |player: &mut Player, seconds: f64| -> (u32, f64) {
        let (began, mut shown, mut last) = (Instant::now(), 0, f64::NAN);
        while began.elapsed().as_secs_f64() < seconds && !player.ended() {
            if let (Some(picture), true) = player.picture() {
                shown += 1;
                last = picture.time;
            }
            std::thread::sleep(Duration::from_millis(4));
        }
        (shown, last)
    };
    for (seconds, to) in steps {
        if seconds > 0.0 {
            let (shown, last) = play(&mut player, seconds);
            println!("played {seconds} s: {shown} pictures, the last at {last:.3}; length {}", length(&player));
        }
        player.seek(to);
        let asked = Instant::now();
        let first = loop {
            if let (Some(picture), true) = player.picture() {
                break Some(picture.time);
            }
            if player.ended() || asked.elapsed() > Duration::from_secs(10) {
                break None;
            }
            std::thread::sleep(Duration::from_millis(2));
        };
        match first {
            Some(time) => println!("  to {to}: first picture at {time:.3} after {:.0} ms; the film is at {:.3}; length {}", asked.elapsed().as_secs_f64() * 1000.0, player.position(), length(&player)),
            None => println!("  to {to}: no picture came; the film is at {:.3}, ended {}", player.position(), player.ended()),
        }
    }
    let began = Instant::now();
    let (shown, last) = play(&mut player, 600.0);
    println!("played to the end in {:.2} s: {shown} pictures, the last at {last:.3}; length {}", began.elapsed().as_secs_f64(), length(&player));
    Ok(())
}
