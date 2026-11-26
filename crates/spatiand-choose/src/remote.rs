//! The remotes, away from the window: the storage service (`storage/`, through
//! `spatiand_storage`) is asked on threads of their own, and what it answers is told back down a
//! channel with a byte down a socket that the window's loop watches (`App::watch`). Nothing the
//! window draws waits on the network, nor on the service being started (up to 8 s).

use std::io::{Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;
use spatiand_storage::{Entry, Progress, Remote, Storage};

use crate::place::Place;

/// The storage service as the chooser and its sender ask it: [`Service`], or a stand-in in the
/// tests.
pub trait Remotes: Send + Sync {
    fn remotes(&self) -> Result<Vec<Remote>, String>;
    fn list(&self, remote: &str, path: &str) -> Result<Vec<Entry>, String>;
    /// What is at a place: nothing, a file (`Some(false)`) or a folder (`Some(true)`).
    fn look(&self, remote: &str, path: &str) -> Result<Option<bool>, String>;
    /// A file, or a folder with everything in it, copied into `into` (a folder of this machine)
    /// as `name`. `progress` is told how it goes, and stops it by answering false: `Ok(true)`
    /// once it is all there, `Ok(false)` once it was stopped.
    fn fetch(&self, remote: &str, path: &str, folder: bool, into: &Path, name: &str, progress: &mut dyn FnMut(&Progress) -> bool) -> Result<bool, String>;
    /// A file or a folder of this machine moved to `remote:path`: made there, or written over
    /// what is there by the name (a folder's things into a folder of its name).
    fn put(&self, from: &Path, folder: bool, remote: &str, path: &str) -> Result<(), String>;
}

/// How often a job of rclone's is looked at, as SpatiShelf looks at its copies.
pub const EVERY: Duration = Duration::from_millis(400);

/// A remote's path split into its folder and its name.
fn split(path: &str) -> (&str, &str) {
    path.trim_matches('/').rsplit_once('/').unwrap_or(("", path.trim_matches('/')))
}

/// The storage service itself: connected to when first asked (started if nobody has), and the
/// connection kept -- let go of when a call fails, so that a service started again since is
/// found again.
#[derive(Default)]
pub struct Service {
    kept: Mutex<Option<Storage>>,
}

impl Service {
    pub(crate) fn with<T>(&self, call: impl FnOnce(&Storage) -> Result<T, String>) -> Result<T, String> {
        let storage = {
            let mut kept = self.kept.lock().unwrap_or_else(|e| e.into_inner());
            match kept.as_ref() {
                Some(storage) => storage.clone(),
                None => {
                    let storage = Storage::connect()?;
                    *kept = Some(storage.clone());
                    storage
                }
            }
        };
        let answer = call(&storage);
        if answer.is_err() {
            *self.kept.lock().unwrap_or_else(|e| e.into_inner()) = None;
        }
        answer
    }
}

impl Remotes for Service {
    fn remotes(&self) -> Result<Vec<Remote>, String> {
        self.with(|s| s.remotes())
    }

    fn list(&self, remote: &str, path: &str) -> Result<Vec<Entry>, String> {
        self.with(|s| s.list(remote, path))
    }

    fn look(&self, remote: &str, path: &str) -> Result<Option<bool>, String> {
        // A remote's top is a folder; rclone is asked only of what is in one.
        if path.trim_matches('/').is_empty() {
            return Ok(Some(true));
        }
        self.with(|s| s.stat(remote, path)).map(|found| found.map(|entry| entry.is_dir))
    }

    fn fetch(&self, remote: &str, path: &str, folder: bool, into: &Path, name: &str, progress: &mut dyn FnMut(&Progress) -> bool) -> Result<bool, String> {
        if folder {
            // Made first, so that an empty folder fetched is a folder there too.
            let to = into.join(name);
            std::fs::create_dir_all(&to).map_err(|e| e.to_string())?;
            let input = json!({"srcFs": format!("{remote}:{}", path.trim_matches('/')), "dstFs": to.to_string_lossy(), "createEmptySrcDirs": true});
            self.with(|s| s.job("sync/copy", input, EVERY, |p| progress(p)))
        } else {
            let (dir, leaf) = split(path);
            let input = json!({"srcFs": format!("{remote}:{dir}"), "srcRemote": leaf, "dstFs": into.to_string_lossy(), "dstRemote": name});
            self.with(|s| s.job("operations/copyfile", input, EVERY, |p| progress(p)))
        }
    }

    fn put(&self, from: &Path, folder: bool, remote: &str, path: &str) -> Result<(), String> {
        let (dir, leaf) = split(path);
        let input = if folder {
            json!({"srcFs": from.to_string_lossy(), "dstFs": format!("{remote}:{}", path.trim_matches('/')), "createEmptySrcDirs": true, "deleteEmptySrcDirs": true})
        } else {
            let (Some(from_dir), Some(from_name)) = (from.parent(), from.file_name()) else { return Err(format!("{} is not a file", from.display())) };
            json!({"srcFs": from_dir.to_string_lossy(), "srcRemote": from_name.to_string_lossy(), "dstFs": format!("{remote}:{dir}"), "dstRemote": leaf})
        };
        let method = if folder { "sync/move" } else { "operations/movefile" };
        self.with(|s| {
            let done = s.job(method, input, EVERY, |_| true);
            // Read straight after through the service's WebDAV (SpatiLoupe, SpatiReel), the
            // folder is as it now is, not as it was half a minute ago.
            s.forget_folders(remote, &[dir]);
            done.map(|_| ())
        })
    }
}

/// What the window asks.
#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    Remotes,
    /// A remote folder's things, for the folder on show as it was when asked (`generation`).
    List { generation: u64, place: Place },
    /// What is at a place, for the question numbered `token`.
    Look { token: u64, place: Place },
    /// The chosen as paths of this machine: a remote's fetched into `into`, a folder of the
    /// chooser's own (`own.rs`); this machine's as they are. `folder`: the chosen are folders,
    /// fetched with everything in them.
    Fetch { chosen: Vec<Place>, folder: bool, into: PathBuf },
}

/// What it is told.
#[derive(Debug, Clone, PartialEq)]
pub enum Answer {
    Remotes(Result<Vec<Remote>, String>),
    Listed { generation: u64, result: Result<Vec<Entry>, String> },
    Looked { token: u64, result: Result<Option<bool>, String> },
    /// The `index`th of `of` being fetched, and how far it has come.
    Fetching { index: usize, of: usize, name: String, progress: Progress },
    /// The fetch is over: the chosen as paths of this machine, in the order they were chosen;
    /// `None` if it was stopped.
    Fetched(Result<Option<Vec<PathBuf>>, String>),
}

/// The thread the storage service is asked from, as the window sees it.
pub struct Side {
    requests: Sender<Request>,
    answers: Receiver<Answer>,
    /// The window's end of the socket a byte comes down with each answer.
    wake: UnixStream,
    /// Set to stop the fetch under way.
    pub stop: Arc<AtomicBool>,
}

impl Side {
    pub fn start(remotes: Arc<dyn Remotes>) -> std::io::Result<Side> {
        let (ours, theirs) = UnixStream::pair()?;
        ours.set_nonblocking(true)?;
        // Never held up by a window that has not read for a while: a byte waiting is enough.
        theirs.set_nonblocking(true)?;
        let theirs = Arc::new(theirs);
        let (requests, asked) = channel::<Request>();
        let (tell, answers) = channel::<Answer>();
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        std::thread::Builder::new().name("remotes".into()).spawn(move || {
            // Each on a thread of its own: a remote that never answers keeps nothing else
            // waiting, a listing not even a fetch.
            for request in asked {
                let (remotes, tell, theirs, stop) = (remotes.clone(), tell.clone(), theirs.clone(), stopping.clone());
                std::thread::spawn(move || {
                    let mut say = |answer: Answer| {
                        if tell.send(answer).is_ok() {
                            let _ = (&*theirs).write(&[1]);
                        }
                    };
                    answer(&*remotes, request, &stop, &mut say);
                });
            }
        })?;
        Ok(Side { requests, answers, wake: ours, stop })
    }

    /// What the window's loop watches.
    pub fn fd(&self) -> RawFd {
        self.wake.as_raw_fd()
    }

    pub fn ask(&self, request: Request) {
        let _ = self.requests.send(request);
    }

    /// What has been answered since last asked, the bytes that woke the window read away.
    pub fn heard(&self) -> Vec<Answer> {
        let mut bytes = [0u8; 256];
        while matches!((&self.wake).read(&mut bytes), Ok(n) if n > 0) {}
        self.answers.try_iter().collect()
    }

    /// The next answer, waited for as long as `wait`: for a window closing on a fetch that is
    /// being stopped, and for the tests.
    pub fn next(&self, wait: Duration) -> Option<Answer> {
        self.answers.recv_timeout(wait).ok()
    }
}

fn answer(remotes: &dyn Remotes, request: Request, stop: &AtomicBool, say: &mut dyn FnMut(Answer)) {
    match request {
        Request::Remotes => say(Answer::Remotes(remotes.remotes())),
        Request::List { generation, place } => {
            let result = match &place {
                Place::Remote(remote, path) => remotes.list(remote, path),
                Place::Local(_) => Err("not a remote".into()),
            };
            say(Answer::Listed { generation, result });
        }
        Request::Look { token, place } => {
            let result = match &place {
                Place::Remote(remote, path) => remotes.look(remote, path),
                Place::Local(path) => Ok(path.symlink_metadata().ok().map(|m| m.is_dir())),
            };
            say(Answer::Looked { token, result });
        }
        Request::Fetch { chosen, folder, into } => {
            let result = fetch(remotes, &chosen, folder, &into, stop, say);
            say(Answer::Fetched(result));
        }
    }
}

/// The chosen as paths of this machine: each of a remote's copied into `into` under its own
/// name -- beside the others, or in a numbered folder of its own where one of them has the name
/// already -- and this machine's as they are, in the order they were chosen.
fn fetch(remotes: &dyn Remotes, chosen: &[Place], folder: bool, into: &Path, stop: &AtomicBool, say: &mut dyn FnMut(Answer)) -> Result<Option<Vec<PathBuf>>, String> {
    let of = chosen.iter().filter(|p| p.is_remote()).count();
    let mut paths = Vec::new();
    let mut index = 0;
    for place in chosen {
        let (remote, path) = match place {
            Place::Local(path) => {
                paths.push(path.clone());
                continue;
            }
            Place::Remote(remote, path) => (remote, path),
        };
        index += 1;
        let name = place.name();
        let mut dir = into.to_path_buf();
        let mut n = 1;
        while dir.join(&name).symlink_metadata().is_ok() || dir.symlink_metadata().is_ok_and(|m| !m.is_dir()) {
            n += 1;
            dir = into.join(n.to_string());
        }
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let before: Vec<_> = std::fs::read_dir(&dir).map(|d| d.flatten().map(|e| e.file_name()).collect()).unwrap_or_default();
        say(Answer::Fetching { index, of, name: name.clone(), progress: Progress::default() });
        let mut progress = |progress: &Progress| {
            say(Answer::Fetching { index, of, name: name.clone(), progress: progress.clone() });
            !stop.load(Ordering::SeqCst)
        };
        if !remotes.fetch(remote, path, folder, &dir, &name, &mut progress)? || stop.load(Ordering::SeqCst) {
            return Ok(None);
        }
        // Where rclone put it: by its name, or by the name a disk here gives it (a character it
        // may not have, written another way) -- the one thing that was not there before.
        let fetched = dir.join(&name);
        if fetched.symlink_metadata().is_ok() {
            paths.push(fetched);
            continue;
        }
        let arrived: Vec<PathBuf> = std::fs::read_dir(&dir)
            .map(|d| d.flatten().filter(|e| !before.contains(&e.file_name()) && e.file_name() != crate::own::PLACE_FILE).map(|e| e.path()).collect())
            .unwrap_or_default();
        match arrived.as_slice() {
            [one] => paths.push(one.clone()),
            _ => return Err(format!("{name} was fetched, but is not where it was fetched to")),
        }
    }
    Ok(Some(paths))
}

/// A stand-in for the storage service in the tests: a remote that is a folder of this machine,
/// as rclone's `alias` is.
#[cfg(test)]
pub mod stand_in {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    pub struct Folder {
        pub name: String,
        pub kind: String,
        pub root: PathBuf,
        /// Each fetch takes its time, told in steps, until it is stopped.
        pub slow: bool,
        /// The service not to be reached.
        pub unreachable: Option<String>,
        /// How many sends are refused before one goes.
        pub refusals: AtomicUsize,
        /// What was asked, in order.
        pub asked: Mutex<Vec<String>>,
    }

    impl Folder {
        pub fn new(name: &str, root: &Path) -> Folder {
            Folder { name: name.into(), kind: "alias".into(), root: root.to_path_buf(), slow: false, unreachable: None, refusals: AtomicUsize::new(0), asked: Mutex::new(Vec::new()) }
        }

        fn at(&self, remote: &str, path: &str) -> Result<PathBuf, String> {
            if let Some(why) = &self.unreachable {
                return Err(why.clone());
            }
            if remote != self.name {
                return Err("didn't find section in config file".into());
            }
            Ok(self.root.join(path.trim_matches('/')))
        }
    }

    pub fn copy(from: &Path, to: &Path) -> std::io::Result<()> {
        if from.is_dir() {
            std::fs::create_dir_all(to)?;
            for entry in std::fs::read_dir(from)? {
                let entry = entry?;
                copy(&entry.path(), &to.join(entry.file_name()))?;
            }
            Ok(())
        } else {
            std::fs::copy(from, to).map(|_| ())
        }
    }

    impl Remotes for Folder {
        fn remotes(&self) -> Result<Vec<Remote>, String> {
            match &self.unreachable {
                Some(why) => Err(why.clone()),
                None => Ok(vec![Remote { name: self.name.clone(), kind: self.kind.clone() }]),
            }
        }

        fn list(&self, remote: &str, path: &str) -> Result<Vec<Entry>, String> {
            self.asked.lock().unwrap().push(format!("list {remote}:{path}"));
            let dir = self.at(remote, path)?;
            let read = std::fs::read_dir(&dir).map_err(|_| "directory not found".to_string())?;
            Ok(read
                .flatten()
                .map(|e| {
                    let meta = e.metadata().unwrap();
                    let name = e.file_name().to_string_lossy().into_owned();
                    Entry { path: format!("{}/{name}", path.trim_matches('/')), name, size: if meta.is_dir() { -1 } else { meta.len() as i64 }, is_dir: meta.is_dir(), modified: String::new(), mime: String::new() }
                })
                .collect())
        }

        fn look(&self, remote: &str, path: &str) -> Result<Option<bool>, String> {
            self.asked.lock().unwrap().push(format!("look {remote}:{path}"));
            Ok(self.at(remote, path)?.symlink_metadata().ok().map(|m| m.is_dir()))
        }

        fn fetch(&self, remote: &str, path: &str, folder: bool, into: &Path, name: &str, progress: &mut dyn FnMut(&Progress) -> bool) -> Result<bool, String> {
            self.asked.lock().unwrap().push(format!("fetch {remote}:{path}"));
            let from = self.at(remote, path)?;
            if folder != from.is_dir() {
                return Err("not what was asked".into());
            }
            if self.slow {
                for done in 0..1000 {
                    std::thread::sleep(Duration::from_millis(10));
                    if !progress(&Progress { done, total: 1000, file: None }) {
                        return Ok(false);
                    }
                }
            }
            copy(&from, &into.join(name)).map_err(|e| e.to_string())?;
            let size = from.metadata().map(|m| m.len()).unwrap_or(0);
            progress(&Progress { done: size, total: size, file: Some(name.into()) });
            Ok(true)
        }

        fn put(&self, from: &Path, folder: bool, remote: &str, path: &str) -> Result<(), String> {
            self.asked.lock().unwrap().push(format!("put {}", path));
            let to = self.at(remote, path)?;
            if self.refusals.load(Ordering::SeqCst) > 0 {
                self.refusals.fetch_sub(1, Ordering::SeqCst);
                return Err("connection refused".into());
            }
            if folder != from.is_dir() {
                return Err("not what was asked".into());
            }
            std::fs::create_dir_all(to.parent().unwrap()).map_err(|e| e.to_string())?;
            copy(from, &to).map_err(|e| e.to_string())?;
            if folder { std::fs::remove_dir_all(from) } else { std::fs::remove_file(from) }.map_err(|e| e.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::stand_in::Folder;
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("spatiand-choose-remote-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("remote/album/inner")).unwrap();
        std::fs::create_dir_all(dir.join("remote/b")).unwrap();
        std::fs::write(dir.join("remote/album/one.jpg"), b"one").unwrap();
        std::fs::write(dir.join("remote/album/inner/two.jpg"), b"two").unwrap();
        std::fs::write(dir.join("remote/photo.jpg"), b"first").unwrap();
        std::fs::write(dir.join("remote/b/photo.jpg"), b"second").unwrap();
        std::fs::create_dir_all(dir.join("into")).unwrap();
        dir
    }

    fn heard_until(side: &Side, mut done: impl FnMut(&Answer) -> bool) -> Vec<Answer> {
        let mut all = Vec::new();
        while let Some(answer) = side.next(Duration::from_secs(10)) {
            let over = done(&answer);
            all.push(answer);
            if over {
                return all;
            }
        }
        panic!("no end in sight: {all:?}");
    }

    /// Fetched beside each other, by their own names, a second of the same name in a folder of
    /// its own; this machine's as they are, all in the order chosen; and told as they go.
    #[test]
    fn the_chosen_are_fetched_in_their_order_and_told_as_they_go() {
        let dir = scratch("order");
        let side = Side::start(Arc::new(Folder::new("Fake", &dir.join("remote")))).unwrap();
        let local = dir.join("local.txt");
        std::fs::write(&local, b"here").unwrap();
        let chosen = vec![Place::remote("Fake", "photo.jpg"), Place::Local(local.clone()), Place::remote("Fake", "b/photo.jpg")];
        side.ask(Request::Fetch { chosen, folder: false, into: dir.join("into") });
        let all = heard_until(&side, |a| matches!(a, Answer::Fetched(_)));
        let Some(Answer::Fetched(Ok(Some(paths)))) = all.last() else { panic!("{all:?}") };
        assert_eq!(paths, &vec![dir.join("into/photo.jpg"), local, dir.join("into/2/photo.jpg")]);
        assert_eq!(std::fs::read(&paths[0]).unwrap(), b"first");
        assert_eq!(std::fs::read(&paths[2]).unwrap(), b"second");
        assert!(all.iter().any(|a| matches!(a, Answer::Fetching { index: 2, of: 2, .. })), "{all:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_folder_is_fetched_with_all_that_is_in_it() {
        let dir = scratch("folder");
        let side = Side::start(Arc::new(Folder::new("Fake", &dir.join("remote")))).unwrap();
        side.ask(Request::Fetch { chosen: vec![Place::remote("Fake", "album")], folder: true, into: dir.join("into") });
        let all = heard_until(&side, |a| matches!(a, Answer::Fetched(_)));
        assert_eq!(all.last(), Some(&Answer::Fetched(Ok(Some(vec![dir.join("into/album")])))));
        assert_eq!(std::fs::read(dir.join("into/album/inner/two.jpg")).unwrap(), b"two");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_fetch_stops_when_told() {
        let dir = scratch("stop");
        let mut slow = Folder::new("Fake", &dir.join("remote"));
        slow.slow = true;
        let side = Side::start(Arc::new(slow)).unwrap();
        side.ask(Request::Fetch { chosen: vec![Place::remote("Fake", "photo.jpg")], folder: false, into: dir.join("into") });
        heard_until(&side, |a| matches!(a, Answer::Fetching { progress, .. } if progress.done > 3));
        side.stop.store(true, Ordering::SeqCst);
        let all = heard_until(&side, |a| matches!(a, Answer::Fetched(_)));
        assert_eq!(all.last(), Some(&Answer::Fetched(Ok(None))));
        assert!(!dir.join("into/photo.jpg").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn listings_and_looks_are_answered_by_their_number() {
        let dir = scratch("list");
        let side = Side::start(Arc::new(Folder::new("Fake", &dir.join("remote")))).unwrap();
        side.ask(Request::List { generation: 4, place: Place::remote("Fake", "album") });
        let Some(Answer::Listed { generation: 4, result: Ok(mut entries) }) = side.next(Duration::from_secs(5)) else { panic!() };
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        assert_eq!(entries.iter().map(|e| (e.name.as_str(), e.is_dir)).collect::<Vec<_>>(), vec![("inner", true), ("one.jpg", false)]);
        side.ask(Request::Look { token: 9, place: Place::remote("Fake", "album/one.jpg") });
        assert_eq!(side.next(Duration::from_secs(5)), Some(Answer::Looked { token: 9, result: Ok(Some(false)) }));
        side.ask(Request::Look { token: 10, place: Place::remote("Fake", "album/none.jpg") });
        assert_eq!(side.next(Duration::from_secs(5)), Some(Answer::Looked { token: 10, result: Ok(None) }));
        side.ask(Request::List { generation: 5, place: Place::remote("Elsewhere", "") });
        assert!(matches!(side.next(Duration::from_secs(5)), Some(Answer::Listed { generation: 5, result: Err(_) })));
        // The window is woken for each.
        let mut byte = [0u8; 16];
        assert!(matches!((&side.wake).read(&mut byte), Ok(n) if n > 0));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
