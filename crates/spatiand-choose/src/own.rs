//! The chooser's own folders, in the wearer's cache folder on the disk ([`base`]): `fetched/`, what was chosen in a remote, copied to this machine for the
//! program that asked, which is given paths of this machine only; and `outbox/`, files saved for
//! a remote, which the program writes there and the chooser's sender moves on (`send.rs`).
//!
//! Each choice has a folder of its own in one of them, with what it stands for written in it
//! ([`PLACE_FILE`]: the place it was chosen in), so that a program asking to begin where its last
//! choice was -- in one of these -- begins in that remote instead.

use std::io;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::place::Place;

/// In each folder of its own: the place it stands for, as `Place::shown` writes it.
pub const PLACE_FILE: &str = ".spatiand-place";
/// In an outbox's folder: the sender's process, while it runs.
pub const SENDER_FILE: &str = ".spatiand-sender";

/// What was fetched is kept a day: a page reads a file it was given when it uploads it, which
/// may be a while after it was chosen.
pub const FETCHED_FOR: Duration = Duration::from_secs(24 * 3600);
/// What could not be sent is kept a week, for the wearer to find.
pub const UNSENT_FOR: Duration = Duration::from_secs(7 * 24 * 3600);

/// Where the chooser keeps what it fetched and what waits to be sent: **on the disk**, in the
/// wearer's cache folder -- not the runtime folder, which on the Deck is in memory and a tenth of it
/// (a film fetched from a remote did not fit), and is emptied when the machine starts again, with
/// whatever could not be sent in it. On Android the app's runtime folder is its own cache on the
/// disk already (crates/spatiand-android's jni sets it so).
pub fn base() -> PathBuf {
    let var = |name: &str| std::env::var_os(name).filter(|d| !d.is_empty()).map(PathBuf::from);
    let cache = if cfg!(target_os = "android") {
        var("XDG_RUNTIME_DIR")
    } else if cfg!(target_os = "macos") {
        var("HOME").map(|home| home.join("Library/Caches"))
    } else {
        var("XDG_CACHE_HOME").or_else(|| var("HOME").map(|home| home.join(".cache")))
    };
    cache.unwrap_or_else(std::env::temp_dir).join("spatiand-choose")
}

/// A new folder of its own in `parent` (`fetched/` or `outbox/` of [`base`]), made, with the
/// place it stands for written in it. Named by the time and this process, and a count where two
/// are made in one moment; private to the wearer.
pub fn folder(parent: &Path, stands_for: &Place) -> io::Result<PathBuf> {
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(parent)?;
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    for n in 0..10_000 {
        let dir = parent.join(format!("{stamp:x}-{:x}-{n}", std::process::id()));
        match std::fs::DirBuilder::new().mode(0o700).create(&dir) {
            Ok(()) => {
                std::fs::write(dir.join(PLACE_FILE), stands_for.shown())?;
                return Ok(dir);
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::other("no free name for a folder"))
}

/// The place a path in one of the chooser's own folders stands for: a remote's folder that was
/// chosen in, or saved to. `None` for a path anywhere else; home where what it stood for is not
/// known, rather than a folder of the chooser's own.
pub fn stands_for(path: &Path, base: &Path, home: &Path) -> Option<Place> {
    for kind in ["fetched", "outbox"] {
        let Ok(rest) = path.strip_prefix(base.join(kind)) else { continue };
        let known = rest
            .components()
            .next()
            .and_then(|own| std::fs::read_to_string(base.join(kind).join(own).join(PLACE_FILE)).ok())
            .and_then(|text| Place::parse(&text))
            .filter(|place| !matches!(place, Place::Local(p) if !p.is_dir()));
        return Some(known.unwrap_or_else(|| Place::Local(home.to_path_buf())));
    }
    None
}

/// Whether the sender of an outbox's folder is running.
pub fn sender_alive(folder: &Path) -> bool {
    let pid = std::fs::read_to_string(folder.join(SENDER_FILE)).ok().and_then(|t| t.trim().parse::<i32>().ok()).filter(|&pid| pid > 0);
    let Some(pid) = pid else { return false };
    let answered = unsafe { libc::kill(pid, 0) } == 0;
    answered || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// What earlier choosers left, taken away: fetched copies a day old, and an outbox's folders a
/// week old whose sender is not running (what a sender could not move stays that long, and its
/// log says where). By the folder's own time, which is when something last arrived in it or
/// left it: a fetched file keeps its remote's time, which may be years ago.
pub fn tidy(base: &Path, now: SystemTime) {
    for (kind, kept) in [("fetched", FETCHED_FOR), ("outbox", UNSENT_FOR)] {
        let Ok(entries) = std::fs::read_dir(base.join(kind)) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = path.symlink_metadata() else { continue };
            let old = meta.modified().ok().and_then(|t| now.duration_since(t).ok()).is_some_and(|age| age > kept);
            if !old || (kind == "outbox" && sender_alive(&path)) {
                continue;
            }
            let gone = if meta.is_dir() { std::fs::remove_dir_all(&path) } else { std::fs::remove_file(&path) };
            match gone {
                Ok(()) => log::info!("took away {}, left from {}", path.display(), if kind == "fetched" { "a fetch" } else { "a save that was not sent" }),
                Err(e) => log::warn!("could not take away {}: {e}", path.display()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("spatiand-choose-own-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_folder_of_its_own_stands_for_where_it_was_chosen() {
        let base = scratch("stands");
        let home = base.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let photos = Place::remote("Drive", "Photos/2024");
        let fetched = folder(&base.join("fetched"), &photos).unwrap();
        let other = folder(&base.join("fetched"), &photos).unwrap();
        assert_ne!(fetched, other, "each its own");
        assert_eq!(stands_for(&fetched, &base, &home), Some(photos.clone()));
        assert_eq!(stands_for(&fetched.join("2"), &base, &home), Some(photos.clone()), "deeper in it too");
        let saved = folder(&base.join("outbox"), &Place::remote("NAS", "")).unwrap();
        assert_eq!(stands_for(&saved, &base, &home), Some(Place::remote("NAS", "")));
        // Not known: home, not a folder of the chooser's own.
        std::fs::create_dir_all(base.join("outbox/stray")).unwrap();
        assert_eq!(stands_for(&base.join("outbox/stray"), &base, &home), Some(Place::Local(home.clone())));
        assert_eq!(stands_for(&home, &base, &home), None, "anywhere else is itself");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn what_earlier_choosers_left_is_taken_away_in_its_time() {
        let base = scratch("tidy");
        let place = Place::remote("Drive", "");
        let fetched = folder(&base.join("fetched"), &place).unwrap();
        let sending = folder(&base.join("outbox"), &place).unwrap();
        let unsent = folder(&base.join("outbox"), &place).unwrap();
        std::fs::write(sending.join(SENDER_FILE), std::process::id().to_string()).unwrap();
        // A sender long gone: no process has a pid this high.
        std::fs::write(unsent.join(SENDER_FILE), "2147483600").unwrap();
        std::fs::write(unsent.join("page.html"), b"kept").unwrap();
        assert!(sender_alive(&sending) && !sender_alive(&unsent));
        let now = SystemTime::now();
        tidy(&base, now);
        assert!(fetched.exists() && unsent.exists(), "new: kept");
        tidy(&base, now + FETCHED_FOR + Duration::from_secs(60));
        assert!(!fetched.exists(), "a day old: gone");
        assert!(unsent.exists(), "what was not sent is kept a week");
        tidy(&base, now + UNSENT_FOR + Duration::from_secs(60));
        assert!(!unsent.exists());
        assert!(sending.exists(), "its sender still at it");
        let _ = std::fs::remove_dir_all(&base);
    }
}
