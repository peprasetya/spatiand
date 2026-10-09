//! The Mac's compositor with no app around it.
//!
//! * `SPATIAND_BACKEND=snapshot` (the default) and the rest of the Deck's snapshot settings draw
//!   one frame to a PNG: see `backend_snapshot.rs`.
//! * `SPATIAND_BACKEND=live` runs the real session, drawing into nothing, for a few seconds, and
//!   plays it a script on standard input, one step a line: `steam`, `quick`, `a`, `b`, `up`,
//!   `down`, `left`, `right`, `move DX DY`, `click`, `key MACCODE`, `shot`, `wait MS`. Pictures
//!   land in `~/screenshots`. With `SPATIAND_HMD=null` there are glasses to draw for.

#[cfg(target_os = "macos")]
fn main() {
    use spatiand_mac::mac::ffi::*;
    if std::env::var("SPATIAND_BACKEND").as_deref() != Ok("live") {
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info,smithay=warn,tracing::span=warn")).init();
        if let Err(e) = spatiand_mac::snapshot() {
            log::error!("snapshot failed: {e}");
            std::process::exit(1);
        }
        return;
    }
    let size = std::env::var("SPATIAND_SNAPSHOT_SIZE").ok();
    let (w, h) = size
        .as_deref()
        .and_then(|s| s.split_once('x'))
        .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
        .unwrap_or((3840, 1080));
    sp_begin(None, std::ptr::null_mut());
    unsafe { sp_glasses(std::ptr::null_mut(), w, h) };
    let wait = |ms: u64| std::thread::sleep(std::time::Duration::from_millis(ms));
    wait(1500);
    let press = |control: i32| {
        sp_control(control, true);
        wait(60);
        sp_control(control, false);
        wait(250);
    };
    for line in std::io::stdin().lines().map_while(Result::ok) {
        let words: Vec<&str> = line.split_whitespace().collect();
        let number = |i: usize| words.get(i).and_then(|w| w.parse::<f32>().ok()).unwrap_or(0.0);
        match words.first().copied() {
            Some("steam") => press(0),
            Some("quick") => press(1),
            Some("a") => press(2),
            Some("b") => press(3),
            Some("up") => press(6),
            Some("down") => press(7),
            Some("left") => press(8),
            Some("right") => press(9),
            Some("move") => {
                sp_pointer(number(1), number(2), 0, 0.0, 0.0);
                wait(120);
            }
            Some("click") => {
                sp_pointer(0.0, 0.0, 1, 0.0, 0.0);
                wait(80);
                sp_pointer(0.0, 0.0, 0, 0.0, 0.0);
                wait(250);
            }
            Some("key") => {
                sp_key(number(1) as u16, true);
                wait(40);
                sp_key(number(1) as u16, false);
                wait(120);
            }
            Some("shot") => {
                sp_screenshot();
                wait(700);
            }
            Some("wait") => wait(number(1) as u64),
            _ => {}
        }
    }
    sp_end();
    wait(300);
}

#[cfg(not(target_os = "macos"))]
fn main() {}
