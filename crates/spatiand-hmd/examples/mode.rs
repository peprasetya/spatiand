//! Put the glasses in one mode after another, a pause between: `mode mono stereo`.
use spatiand_hmd::{DisplayMode, Hmd, XrealGlasses};
fn main() {
    let mut glasses = XrealGlasses::open_any().expect("no glasses");
    let hz = std::env::var("HZ").ok().and_then(|v| v.parse().ok()).unwrap_or(60);
    glasses.prefer_refresh(hz);
    for word in std::env::args().skip(1) {
        let mode = if word == "stereo" { DisplayMode::Stereo } else { DisplayMode::Mono };
        println!("{word}: {:?}", glasses.set_display_mode(mode));
        std::thread::sleep(std::time::Duration::from_secs(6));
    }
    std::mem::forget(glasses);
}
