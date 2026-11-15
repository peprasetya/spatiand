//! The folders on show, watched: whatever changes one -- another SpatiShelf window, another
//! program, the trash -- the tab says so, and the window lists it again, keeping its cursor,
//! what is chosen, the scroll and what is open as any listing again does.
//!
//! One mechanism on every system, the `notify` crate's: inotify on Linux and Android, FSEvents
//! on the Mac. Each folder is watched by itself, not what is inside it (what is opened inside
//! is watched as a folder of its own). Changes come in bursts -- a copy writes a file a piece at
//! a time -- so a folder is said to have changed a quarter of a second after the last change
//! of a burst, and at most once a second while one goes on. A remote cannot be watched: the
//! window lists the remote folder on show again every [`REMOTE_EVERY`] instead.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use notify::{EventKind, RecursiveMode, Watcher};

use crate::proto::Place;

/// Quiet for this long after a change, a folder is said to have changed.
pub const SETTLE: Duration = Duration::from_millis(250);
/// While changes go on, it is said at least this often.
pub const AT_MOST: Duration = Duration::from_millis(1000);
/// How often the remote folder on show is listed again.
pub const REMOTE_EVERY: Duration = Duration::from_secs(15);

/// Changes gathered per place, each to be said once it has settled.
#[derive(Debug, Default)]
pub struct Debounce {
    pending: HashMap<Place, (Instant, Instant)>,
}

impl Debounce {
    pub fn changed(&mut self, place: Place, now: Instant) {
        self.pending.entry(place).and_modify(|(_, last)| *last = now).or_insert((now, now));
    }

    fn due_at(first: Instant, last: Instant) -> Instant {
        (last + SETTLE).min(first + AT_MOST)
    }

    /// How long until the next is due; none while nothing waits.
    pub fn wait(&self, now: Instant) -> Option<Duration> {
        self.pending.values().map(|&(f, l)| Self::due_at(f, l).saturating_duration_since(now)).min()
    }

    /// The places due now, taken.
    pub fn due(&mut self, now: Instant) -> Vec<Place> {
        let due: Vec<Place> = self.pending.iter().filter(|(_, &(f, l))| Self::due_at(f, l) <= now).map(|(p, _)| p.clone()).collect();
        for p in &due {
            self.pending.remove(p);
        }
        due
    }

    /// A place no longer watched: what it had waiting is not said.
    pub fn forget(&mut self, keep: &HashSet<Place>) {
        self.pending.retain(|p, _| keep.contains(p));
    }
}

/// The folders on this machine a place is, to be watched: a folder itself, the trash's.
pub fn folders_of(place: &Place) -> Vec<PathBuf> {
    match place {
        Place::Local(p) => vec![PathBuf::from(p)],
        Place::Trash => crate::trash::folders(),
        Place::Remote(..) => Vec::new(),
    }
}

/// Which watched place a changed path is news for: the folder it is in, or the folder itself.
pub fn place_for(path: &std::path::Path, watched: &HashMap<PathBuf, Place>) -> Option<Place> {
    watched.get(path).or_else(|| path.parent().and_then(|p| watched.get(p))).cloned()
}

enum Message {
    Watch(Vec<Place>),
    Event(notify::Result<notify::Event>),
}

/// What the tab tells its watcher.
pub struct Watching {
    tell: Sender<Message>,
}

impl Watching {
    /// Watch these places (and no others).
    pub fn set(&self, places: Vec<Place>) {
        let _ = self.tell.send(Message::Watch(places));
    }
}

/// Start the watcher: `changed` is told each place that changed, once it settled.
pub fn start(changed: impl Fn(Place) -> bool + Send + 'static) -> Watching {
    let (tell, heard) = channel();
    let events = tell.clone();
    std::thread::spawn(move || run(heard, events, changed));
    Watching { tell }
}

fn run(heard: Receiver<Message>, events: Sender<Message>, changed: impl Fn(Place) -> bool) {
    let mut watcher = match notify::recommended_watcher(move |e| {
        let _ = events.send(Message::Event(e));
    }) {
        Ok(w) => Some(w),
        Err(e) => {
            log::warn!("folders will not be watched: {e}");
            None
        }
    };
    // Each folder watched, and the place it is news for.
    let mut watched: HashMap<PathBuf, Place> = HashMap::new();
    let mut bounce = Debounce::default();
    loop {
        let message = match bounce.wait(Instant::now()) {
            Some(wait) => heard.recv_timeout(wait),
            None => heard.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        match message {
            Ok(Message::Watch(places)) => {
                let mut wanted: HashMap<PathBuf, Place> = HashMap::new();
                for place in &places {
                    for folder in folders_of(place) {
                        wanted.insert(folder, place.clone());
                    }
                }
                if let Some(w) = watcher.as_mut() {
                    for gone in watched.keys().filter(|f| !wanted.contains_key(*f)) {
                        let _ = w.unwatch(gone);
                    }
                    for (folder, _) in wanted.iter().filter(|(f, _)| !watched.contains_key(*f)) {
                        if let Err(e) = w.watch(folder, RecursiveMode::NonRecursive) {
                            log::debug!("{} is not watched: {e}", folder.display());
                        }
                    }
                }
                watched = wanted;
                bounce.forget(&places.into_iter().collect());
            }
            Ok(Message::Event(Ok(event))) => {
                // Reading a file changes nothing on show.
                if matches!(event.kind, EventKind::Access(_)) {
                    continue;
                }
                let now = Instant::now();
                for path in &event.paths {
                    if let Some(place) = place_for(path, &watched) {
                        bounce.changed(place, now);
                    }
                }
                // Too many at once for the kernel to keep: everything watched may have changed.
                if event.need_rescan() {
                    for place in watched.values() {
                        bounce.changed(place.clone(), now);
                    }
                }
            }
            Ok(Message::Event(Err(e))) => log::debug!("watching: {e}"),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        for place in bounce.due(Instant::now()) {
            if !changed(place) {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(p: &str) -> Place {
        Place::Local(p.into())
    }

    #[test]
    fn a_burst_is_said_once_it_settles_and_a_long_one_every_second() {
        let t0 = Instant::now();
        let mut b = Debounce::default();
        assert_eq!(b.wait(t0), None);
        b.changed(at("/a"), t0);
        b.changed(at("/a"), t0 + Duration::from_millis(100));
        assert_eq!(b.wait(t0 + Duration::from_millis(100)), Some(SETTLE));
        assert!(b.due(t0 + Duration::from_millis(300)).is_empty());
        assert_eq!(b.due(t0 + Duration::from_millis(350)), [at("/a")]);
        assert!(b.due(t0 + Duration::from_secs(5)).is_empty(), "once");
        // A change every 100 ms: said after a second all the same.
        for i in 0..15 {
            b.changed(at("/b"), t0 + Duration::from_millis(2000 + i * 100));
        }
        assert_eq!(b.due(t0 + Duration::from_millis(3000)), [at("/b")]);
        // A place no longer watched: forgotten.
        b.changed(at("/c"), t0);
        b.forget(&HashSet::new());
        assert!(b.due(t0 + Duration::from_secs(9)).is_empty());
    }

    #[test]
    fn news_for_the_folder_a_change_is_in() {
        let watched: HashMap<PathBuf, Place> = [(PathBuf::from("/r"), at("/r")), (PathBuf::from("/r/Photos"), at("/r/Photos")), (PathBuf::from("/t/files"), Place::Trash)].into();
        assert_eq!(place_for(std::path::Path::new("/r/a.txt"), &watched), Some(at("/r")));
        assert_eq!(place_for(std::path::Path::new("/r/Photos"), &watched), Some(at("/r/Photos")), "the folder itself");
        assert_eq!(place_for(std::path::Path::new("/r/Photos/x.jpg"), &watched), Some(at("/r/Photos")));
        assert_eq!(place_for(std::path::Path::new("/t/files/old"), &watched), Some(Place::Trash));
        assert_eq!(place_for(std::path::Path::new("/r/Music/y.mp3"), &watched), None, "not opened, not watched");
    }

    #[test]
    fn a_file_made_by_anyone_is_heard() {
        let dir = std::env::temp_dir().join(format!("spatishelf-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("inner")).unwrap();
        let (tx, rx) = channel();
        let watching = start(move |p| tx.send(p).is_ok());
        let place = Place::Local(dir.to_string_lossy().into_owned());
        let inner = Place::Local(dir.join("inner").to_string_lossy().into_owned());
        watching.set(vec![place.clone(), inner.clone()]);
        std::thread::sleep(Duration::from_millis(200));
        std::fs::write(dir.join("made.txt"), b"x").unwrap();
        assert_eq!(rx.recv_timeout(Duration::from_secs(3)), Ok(place.clone()));
        std::fs::rename(dir.join("made.txt"), dir.join("inner/made.txt")).unwrap();
        let mut heard: Vec<Place> = (0..2).filter_map(|_| rx.recv_timeout(Duration::from_secs(3)).ok()).collect();
        heard.sort_by_key(|p| p.text());
        assert_eq!(heard, [place.clone(), inner.clone()], "a move is news at both ends");
        // No longer watched: nothing.
        watching.set(vec![place]);
        std::thread::sleep(Duration::from_millis(200));
        std::fs::write(dir.join("inner/quiet.txt"), b"x").unwrap();
        assert!(rx.recv_timeout(Duration::from_millis(800)).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
