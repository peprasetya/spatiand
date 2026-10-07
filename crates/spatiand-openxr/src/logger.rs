//! Where the runtime says what it is doing.
//!
//! A runtime is loaded into somebody else's process, often one whose standard error goes nowhere
//! anyone will read (a game under Proton, for one). So it writes a file: `SPATIAND_OPENXR_LOG`
//! names it, and otherwise it is `~/.local/share/spatiand-openxr.log`: the home directory is the one
//! place a container around a game (Steam's, for one) shares with the rest of the system, where
//! `/tmp` and `XDG_RUNTIME_DIR` are private to it and a log written there is never found. Setting
//! `SPATIAND_OPENXR_STDERR` mirrors the lines to standard error as well.

use std::io::Write;
use std::sync::{Mutex, Once};

struct Logger {
    file: Mutex<Option<std::fs::File>>,
    stderr: bool,
}

impl log::Log for Logger {
    fn enabled(&self, _: &log::Metadata) -> bool {
        true
    }

    fn log(&self, record: &log::Record) {
        let line = format!("[{:>5}] {}\n", record.level(), record.args());
        if let Some(file) = self.file.lock().unwrap().as_mut() {
            let _ = file.write_all(line.as_bytes());
        }
        if self.stderr {
            eprint!("spatiand-openxr {line}");
        }
    }

    fn flush(&self) {}
}

static START: Once = Once::new();

pub fn init() {
    START.call_once(|| {
        let path = std::env::var("SPATIAND_OPENXR_LOG").unwrap_or_else(|_| {
            match std::env::var("HOME") {
                Ok(home) => format!("{home}/.local/share/spatiand-openxr.log"),
                Err(_) => "/tmp/spatiand-openxr.log".into(),
            }
        });
        let file = std::fs::OpenOptions::new().create(true).append(true).open(path).ok();
        let logger = Box::leak(Box::new(Logger {
            file: Mutex::new(file),
            stderr: std::env::var_os("SPATIAND_OPENXR_STDERR").is_some(),
        }));
        let _ = log::set_logger(logger);
        log::set_max_level(log::LevelFilter::Debug);
        log::info!("spatiand-openxr {} loaded in pid {}", env!("CARGO_PKG_VERSION"), std::process::id());
    });
}
