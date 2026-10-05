//! The Rust half's log lines, to standard error, which the app writes to its log file.
//!
//! Until this existed every `log::info!` in the tracker and the room went nowhere on the Mac, which
//! is why a drifting view had nothing to explain it: the line that says whether the magnetic anchor
//! is holding was being thrown away.

use std::io::Write;
use std::sync::Once;

struct Stderr;

impl log::Log for Stderr {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        // Our own crates say what they like; everyone else only when something is wrong.
        if metadata.target().starts_with("spatiand") {
            metadata.level() <= log::Level::Info
        } else {
            metadata.level() <= log::Level::Warn
        }
    }

    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            let _ = writeln!(std::io::stderr(), "[{}] {}", record.level(), record.args());
        }
    }

    fn flush(&self) {}
}

pub fn init() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        if log::set_logger(&Stderr).is_ok() {
            log::set_max_level(log::LevelFilter::Info);
        }
    });
}
