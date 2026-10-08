//! The Mac's compositor with no app around it. `SPATIAND_BACKEND=snapshot` and the rest of the
//! Deck's snapshot settings draw one frame to a PNG.

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info,smithay::input::keyboard=warn")).init();
    #[cfg(target_os = "macos")]
    if let Err(e) = spatiand_mac::snapshot() {
        log::error!("snapshot failed: {e}");
        std::process::exit(1);
    }
}
