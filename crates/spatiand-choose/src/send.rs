//! The sender: `spatiand-choose --send <file> <remote:path>`, started by the chooser on its own
//! when something is saved to a remote, so that it outlives the chooser and the program that
//! asked.
//!
//! The program (SpatiMesh's Chromium) was given a path of this machine, `<file>`, in a folder of
//! the outbox made for this one save (`own.rs`), and writes it there in its own time: a download
//! as `<name>.crdownload`, renamed to `<name>` once it is whole; a page's File System Access
//! write as `<name>.crswap`, renamed over `<name>` once the page closes it. So the sender waits
//! until the file is there under its name, nothing temporary is beside it, and nothing in the
//! folder has changed for two seconds ([`Rules`]); while a temporary file is there it waits as
//! long as that takes (a download paused); nothing ever written, it gives up after half an hour
//! (the save was cancelled). Then it moves what is there to the remote (rclone's
//! `operations/movefile`), trying again a few times, and takes the folder away. What it could not
//! move stays where it is -- for a week, `own::tidy` -- and its log, `send.log` beside the
//! outbox, says where.
//!
//! It has no window, and says nothing in the room: Spatiand's applications have no way to show
//! the wearer a notice of their own (the room's notice card is for the platform's notifications),
//! so the log is all it can say.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::own;
use crate::place::Place;
use crate::remote::Remotes;

/// How long the sender waits, and for what.
#[derive(Debug, Clone)]
pub struct Rules {
    /// How often the folder is looked at.
    pub every: Duration,
    /// How long nothing in it has to have changed.
    pub settled: Duration,
    /// How long an empty file of its name, never written through a temporary one, is waited on:
    /// the File System Access API makes the file as it is chosen, before the page writes it.
    pub placeholder: Duration,
    /// How long nothing at all is waited on before the save is taken to have been cancelled.
    pub give_up: Duration,
}

impl Default for Rules {
    fn default() -> Rules {
        Rules { every: Duration::from_millis(500), settled: Duration::from_secs(2), placeholder: Duration::from_secs(30), give_up: Duration::from_secs(30 * 60) }
    }
}

/// The pauses before each try after the first.
pub const PAUSES: [Duration; 4] = [Duration::from_secs(5), Duration::from_secs(15), Duration::from_secs(45), Duration::from_secs(120)];

#[derive(Debug, Clone, PartialEq)]
pub enum Waited {
    /// Written, and still: to be sent.
    Ready,
    /// Nothing written in all the time given: the save was cancelled.
    Nothing,
}

/// Something in the folder, as it is at a look: its path in it, whether it is a folder, its
/// size and when it last changed.
#[derive(Debug, Clone, PartialEq)]
struct Thing {
    path: PathBuf,
    folder: bool,
    size: u64,
    changed: Option<std::time::SystemTime>,
}

/// A browser's file not yet whole: Chromium's download, or its File System Access write.
fn temporary(path: &Path) -> bool {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    name.ends_with(".crdownload") || name.ends_with(".crswap")
}

/// Everything in the folder but the chooser's own files, in a steady order.
fn look(folder: &Path) -> Vec<Thing> {
    fn walk(dir: &Path, within: &Path, out: &mut Vec<Thing>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let name = entry.file_name();
            if within.as_os_str().is_empty() && (name == own::PLACE_FILE || name == own::SENDER_FILE) {
                continue;
            }
            let Ok(meta) = entry.path().symlink_metadata() else { continue };
            let path = within.join(&name);
            out.push(Thing { path: path.clone(), folder: meta.is_dir(), size: meta.len(), changed: meta.modified().ok() });
            if meta.is_dir() {
                walk(&entry.path(), &path, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(folder, Path::new(""), &mut out);
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// Wait for the program to have written `name` into `folder`, by the [`Rules`].
pub fn wait(folder: &Path, name: &str, rules: &Rules) -> Waited {
    let mut last_seen = Instant::now();
    let mut seen_temporary = false;
    let mut before: Option<Vec<Thing>> = None;
    let mut same_since = Instant::now();
    loop {
        let now = look(folder);
        let busy = now.iter().any(|t| temporary(&t.path));
        seen_temporary |= busy;
        if !now.is_empty() {
            last_seen = Instant::now();
        }
        if before.as_ref() != Some(&now) {
            before = Some(now.clone());
            same_since = Instant::now();
        }
        let still = same_since.elapsed();
        if !busy && !now.is_empty() && still >= rules.settled {
            match now.iter().find(|t| t.path.as_os_str() == name) {
                Some(t) if t.size == 0 && !t.folder && now.len() == 1 && !seen_temporary => {
                    if still >= rules.placeholder {
                        return Waited::Ready;
                    }
                }
                Some(_) => return Waited::Ready,
                // Written under a name of the program's own (a page saved as `name.html`): sent
                // under it, once all of it has been still a good while.
                None if still >= rules.settled * 5 => return Waited::Ready,
                None => {}
            }
        }
        if now.is_empty() && last_seen.elapsed() >= rules.give_up {
            return Waited::Nothing;
        }
        std::thread::sleep(rules.every);
    }
}

/// Everything in the folder moved to the remote: the file of its name to `to`, anything else the
/// program wrote with it (a page's `_files`) beside it there, under its own name.
fn put_all(remotes: &dyn Remotes, folder: &Path, name: &str, to: &Place) -> Result<(), String> {
    let Place::Remote(remote, path) = to else { return Err(format!("{} is not a remote's", to.shown())) };
    let beside = to.parent().unwrap_or_else(|| to.clone());
    let entries = std::fs::read_dir(folder).map_err(|e| e.to_string())?;
    for entry in entries.flatten() {
        let own = entry.file_name();
        if own == own::PLACE_FILE || own == own::SENDER_FILE {
            continue;
        }
        let own = own.to_string_lossy().into_owned();
        let target = if own == name {
            path.clone()
        } else {
            match beside.join(&own) {
                Place::Remote(_, path) => path,
                Place::Local(_) => unreachable!(),
            }
        };
        let is_folder = entry.path().symlink_metadata().is_ok_and(|m| m.is_dir());
        remotes.put(&entry.path(), is_folder, remote, &target)?;
    }
    Ok(())
}

/// What the program wrote, moved to the remote: tried again after each of `pauses`, as long as
/// it fails.
pub fn send(remotes: &dyn Remotes, folder: &Path, name: &str, to: &Place, pauses: &[Duration]) -> Result<(), String> {
    let mut tries = std::iter::once(Duration::ZERO).chain(pauses.iter().copied()).enumerate().peekable();
    loop {
        let Some((n, pause)) = tries.next() else { unreachable!() };
        std::thread::sleep(pause);
        match put_all(remotes, folder, name, to) {
            Ok(()) => return Ok(()),
            Err(e) if tries.peek().is_none() => return Err(e),
            Err(e) => log::warn!("try {}: {} not sent to {}: {e}", n + 1, name, to.shown()),
        }
    }
}

/// Said on the room's card (`App::notify`), where the wearer hears it: the sender has no window.
/// Nothing, where there is no room to say it in.
fn tell(summary: &str, body: &str) {
    match spatiand_app::App::connect("id.prasetya.spatiand.choose") {
        Ok(mut app) => {
            if let Err(e) = app.notify("Files", summary, body) {
                log::warn!("could not say so in the room: {e}");
            }
        }
        Err(e) => log::warn!("no room to say so in: {e}"),
    }
}

/// `--send <file> <remote:path>`: what the sender does, all of it. Its exit code.
pub fn run(file: &Path, to: &str) -> i32 {
    let base = own::base();
    log_to(&base.join("send.log"));
    let Some(to) = Place::parse(to).filter(Place::is_remote) else {
        log::error!("--send: {to} is not a remote's place");
        return 2;
    };
    let (Some(folder), Some(name)) = (file.parent(), file.file_name().map(|n| n.to_string_lossy().into_owned())) else {
        log::error!("--send: {} is not a file's path", file.display());
        return 2;
    };
    let _ = std::fs::write(folder.join(own::SENDER_FILE), std::process::id().to_string());
    log::info!("waiting for {} to be written, to send it to {}", file.display(), to.shown());
    if wait(folder, &name, &Rules::default()) == Waited::Nothing {
        log::info!("nothing was written to {} in half an hour: the save was cancelled", file.display());
        let _ = std::fs::remove_dir_all(folder);
        return 0;
    }
    let service = crate::remote::Service::default();
    match send(&service, folder, &name, &to, &PAUSES) {
        Ok(()) => {
            log::info!("sent {name} to {}", to.shown());
            let _ = std::fs::remove_dir_all(folder);
            tell(&format!("Saved {name}"), &format!("in {}", to.shown()));
            0
        }
        Err(e) => {
            log::error!("{name} could not be sent to {}: {e}. It is kept in {} for a week.", to.shown(), folder.display());
            let _ = std::fs::remove_file(folder.join(own::SENDER_FILE));
            tell(&format!("{name} could not be saved in {}", to.shown()), &format!("{e}. It is kept in {} for a week.", folder.display()));
            1
        }
    }
}

/// The sender's log: a file of its own, beside the outbox, its standard streams being closed.
/// Begun again when it has grown past a megabyte.
fn log_to(path: &Path) {
    let _ = std::fs::create_dir_all(path.parent().unwrap_or(Path::new("/")));
    if std::fs::metadata(path).is_ok_and(|m| m.len() > 1 << 20) {
        let _ = std::fs::remove_file(path);
    }
    let mut builder = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"));
    if let Ok(file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        builder.target(env_logger::Target::Pipe(Box::new(file)));
    }
    builder.init();
}

/// A sender for `file`, started on its own: in a process group of its own, with nothing of the
/// chooser's open in it -- no standard stream above all, for the program that asked reads the
/// chooser's output to its end, which a sender holding it would put off until it was done.
pub fn start(file: &Path, to: &Place) -> Result<(), String> {
    let program = std::env::current_exe().map_err(|e| format!("this program is not to be found: {e}"))?;
    let mut command = Command::new(program);
    command.arg("--send").arg(file).arg(to.shown());
    command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    use std::os::unix::process::CommandExt;
    command.process_group(0);
    // Measured here: only what may be done between fork and exec is done there.
    let top = (unsafe { libc::sysconf(libc::_SC_OPEN_MAX) } as i32).clamp(64, 65536);
    unsafe {
        command.pre_exec(move || {
            for fd in 3..top {
                libc::close(fd);
            }
            Ok(())
        });
    }
    let child = command.spawn().map_err(|e| e.to_string())?;
    log::info!("sending {} to {} (the sender is {})", file.display(), to.shown(), child.id());
    let _ = std::io::stderr().flush();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::stand_in::Folder;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("spatiand-choose-send-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("remote/Docs")).unwrap();
        dir
    }

    fn quick() -> Rules {
        Rules { every: Duration::from_millis(20), settled: Duration::from_millis(200), placeholder: Duration::from_millis(1200), give_up: Duration::from_millis(800) }
    }

    /// The program's writing, as a thread: each step, then a pause.
    fn browser(steps: Vec<(Duration, Box<dyn FnOnce() + Send>)>) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            for (pause, step) in steps {
                std::thread::sleep(pause);
                step();
            }
        })
    }

    #[test]
    fn a_download_is_waited_for_until_it_is_renamed_whole() {
        let dir = scratch("download");
        let outbox = own::folder(&dir.join("outbox"), &Place::remote("Fake", "Docs")).unwrap();
        let (part, whole) = (outbox.join("report.pdf.crdownload"), outbox.join("report.pdf"));
        let (p, w) = (part.clone(), whole.clone());
        let started = Instant::now();
        let writing = browser(vec![
            (Duration::from_millis(100), Box::new({ let p = p.clone(); move || std::fs::write(&p, b"half").unwrap() })),
            // Longer than it takes to settle: the temporary one is waited on, however long.
            (Duration::from_millis(700), Box::new({ let p = p.clone(); move || std::fs::write(&p, b"half and the rest").unwrap() })),
            (Duration::from_millis(300), Box::new(move || std::fs::rename(&p, &w).unwrap())),
        ]);
        assert_eq!(wait(&outbox, "report.pdf", &quick()), Waited::Ready);
        assert!(started.elapsed() >= Duration::from_millis(1300), "not before the rename and its settling");
        writing.join().unwrap();
        assert!(!part.exists() && whole.exists());
        // And moved there, the folder's own file left behind for the folder to go with it.
        let remote = Folder::new("Fake", &dir.join("remote"));
        send(&remote, &outbox, "report.pdf", &Place::remote("Fake", "Docs/report.pdf"), &[]).unwrap();
        assert_eq!(std::fs::read(dir.join("remote/Docs/report.pdf")).unwrap(), b"half and the rest");
        assert!(!whole.exists() && outbox.join(own::PLACE_FILE).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_page_s_write_waits_for_its_swap_file() {
        let dir = scratch("swap");
        let outbox = own::folder(&dir.join("outbox"), &Place::remote("Fake", "Docs")).unwrap();
        let (swap, file) = (outbox.join("notes.txt.crswap"), outbox.join("notes.txt"));
        // The File System Access API makes the file empty as it is chosen, and the page writes it
        // through its swap file, renamed over it when it is closed.
        std::fs::write(&file, b"").unwrap();
        let started = Instant::now();
        let writing = browser(vec![
            (Duration::from_millis(400), Box::new({ let s = swap.clone(); move || std::fs::write(&s, b"what the page wrote").unwrap() })),
            (Duration::from_millis(400), Box::new({ let (s, f) = (swap.clone(), file.clone()); move || std::fs::rename(&s, &f).unwrap() })),
        ]);
        assert_eq!(wait(&outbox, "notes.txt", &quick()), Waited::Ready);
        assert!(started.elapsed() >= Duration::from_millis(1000), "the empty file was not taken for it");
        writing.join().unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"what the page wrote");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_empty_file_is_sent_only_once_it_has_stayed_empty() {
        let dir = scratch("empty");
        let outbox = own::folder(&dir.join("outbox"), &Place::remote("Fake", "Docs")).unwrap();
        std::fs::write(outbox.join("empty.txt"), b"").unwrap();
        let started = Instant::now();
        assert_eq!(wait(&outbox, "empty.txt", &quick()), Waited::Ready);
        assert!(started.elapsed() >= quick().placeholder);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn nothing_written_is_given_up_and_a_page_saved_under_its_own_name_is_sent_so() {
        let dir = scratch("nothing");
        let outbox = own::folder(&dir.join("outbox"), &Place::remote("Fake", "Docs")).unwrap();
        assert_eq!(wait(&outbox, "never.pdf", &quick()), Waited::Nothing);
        // Saved as a page, with its things: under the name the program gave it, beside the rest.
        std::fs::write(outbox.join("page.html"), b"<html>").unwrap();
        std::fs::create_dir_all(outbox.join("page_files")).unwrap();
        std::fs::write(outbox.join("page_files/a.png"), b"png").unwrap();
        assert_eq!(wait(&outbox, "page", &quick()), Waited::Ready);
        let remote = Folder::new("Fake", &dir.join("remote"));
        send(&remote, &outbox, "page", &Place::remote("Fake", "Docs/page"), &[]).unwrap();
        assert_eq!(std::fs::read(dir.join("remote/Docs/page.html")).unwrap(), b"<html>");
        assert_eq!(std::fs::read(dir.join("remote/Docs/page_files/a.png")).unwrap(), b"png");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_send_refused_is_tried_again_and_one_never_taken_is_left_where_it_is() {
        let dir = scratch("again");
        let outbox = own::folder(&dir.join("outbox"), &Place::remote("Fake", "Docs")).unwrap();
        std::fs::write(outbox.join("a.txt"), b"a").unwrap();
        let remote = Folder::new("Fake", &dir.join("remote"));
        remote.refusals.store(2, std::sync::atomic::Ordering::SeqCst);
        let pauses = [Duration::from_millis(10); 3];
        send(&remote, &outbox, "a.txt", &Place::remote("Fake", "Docs/a.txt"), &pauses).unwrap();
        assert_eq!(std::fs::read(dir.join("remote/Docs/a.txt")).unwrap(), b"a");
        std::fs::write(outbox.join("b.txt"), b"b").unwrap();
        remote.refusals.store(10, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(send(&remote, &outbox, "b.txt", &Place::remote("Fake", "Docs/b.txt"), &pauses), Err("connection refused".into()));
        assert!(outbox.join("b.txt").exists(), "kept");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
