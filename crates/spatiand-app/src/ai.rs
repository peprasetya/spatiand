//! The AI settings every application shares: hemera's address, the models (the everyday one,
//! the larger one, the terminal clean-up one, the one that reads text in pictures), dictation's
//! languages and words, and how text in pictures is translated.
//!
//! The wearer changes them in the room's settings, on its AI card, and they are the same on every
//! paired machine: one file in the shared folder ([`crate::config::shared`]), kept by
//! `spatiand_ai`. [`settings`] reads them; [`crate::Event::AiSettings`] comes from
//! [`crate::App::wait`] when they have changed -- on the card, or on another machine and
//! synchronised here -- and they are then read again.
//!
//! ```no_run
//! let ai = spatiand_app::ai::settings();
//! println!("asking {} with {}", ai.hemera, ai.models.everyday);
//! ```

use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub use spatiand_ai::{AiSettings, Dictation, Models, TextInPictures};

/// The AI settings as they are now.
pub fn settings() -> AiSettings {
    AiSettings::load(&crate::config::shared())
}

/// Whether the settings in a folder have changed since last asked: their file's time and length,
/// compared. One look at the file, no reading of it.
pub(crate) struct Watch {
    folder: PathBuf,
    seen: Option<(SystemTime, u64)>,
}

impl Watch {
    pub(crate) fn new(folder: &Path) -> Watch {
        Watch { folder: folder.to_path_buf(), seen: spatiand_ai::stamp(folder) }
    }

    /// `true` once for each change since the last call.
    pub(crate) fn changed(&mut self) -> bool {
        let now = spatiand_ai::stamp(&self.folder);
        let changed = now != self.seen;
        self.seen = now;
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_application_reads_what_the_card_kept_and_hears_when_it_changes() {
        let dir = std::env::temp_dir().join(format!("spatiand-app-ai-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut watch = Watch::new(&dir);
        assert!(!watch.changed(), "nothing there, nothing changed");
        // The card keeps a choice: the application is told, once, and reads it.
        let mut kept = AiSettings::default();
        kept.models.everyday = "ornith-1.5:9b".into();
        kept.save(&dir).unwrap();
        assert!(watch.changed());
        assert!(!watch.changed());
        assert_eq!(AiSettings::load(&dir).models.everyday, "ornith-1.5:9b");
        // Another, a moment later (a second machine's, synchronised): told again.
        std::thread::sleep(std::time::Duration::from_millis(20));
        kept.hemera = "http://elsewhere:12434".into();
        kept.save(&dir).unwrap();
        assert!(watch.changed());
        assert_eq!(AiSettings::load(&dir).hemera, "http://elsewhere:12434");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
