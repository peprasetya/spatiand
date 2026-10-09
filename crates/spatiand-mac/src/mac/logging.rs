//! Logging, started once whichever way the library is entered: to stderr, which the app sends
//! to its log file.

pub fn init() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = env_logger::Builder::from_env(
            env_logger::Env::default().default_filter_or("info,smithay=warn,tracing::span=warn"),
        )
        .try_init();
        // Say what went wrong before dying, as the Deck's `main` does.
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            log::error!("spatiand is about to die: {info}");
            use std::io::Write;
            let _ = std::io::stderr().flush();
            previous(info);
        }));
    });
}
