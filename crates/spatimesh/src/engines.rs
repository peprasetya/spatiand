//! The engine hosts the window process talks to: one per profile in use (docs/spatimesh.md).

use std::collections::HashMap;
use std::os::fd::OwnedFd;
use std::path::PathBuf;
use std::process::Child;

use spatimesh::engine::{fake::Fake, ProfileSetup, TabId};
use spatimesh::profiles::PRIVATE;
use spatimesh::store::Store;
use spatimesh::wire::{FromHost, Link, Pictures};

/// What draws pages here.
#[derive(Debug, Clone, PartialEq)]
pub enum Program {
    /// An engine program beside this one.
    Path(PathBuf),
    /// Android's WebView, in a service of Spatiand's APK that the launcher in Spatiand's own
    /// process starts when asked on this abstract socket (android/src/.../mesh/MeshLauncher.java).
    #[cfg(any(target_os = "linux", target_os = "android"))]
    Launched(String),
    /// No engine: flat colours, in a thread of this process (`SPATIMESH_ENGINE=fake`, and where
    /// a platform has none yet).
    Fake(String),
}

fn beside(name: &str) -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let name = if cfg!(target_os = "android") { format!("lib{name}.so") } else { name.to_string() };
    let path = dir.join(name);
    path.is_file().then_some(path)
}

/// Whether this Linux is SteamOS, whose own Chromium SpatiMesh uses (docs/spatimesh.md).
pub fn is_steamos() -> bool {
    std::fs::read_to_string("/etc/os-release").is_ok_and(|text| {
        text.lines().any(|line| matches!(line.trim(), "ID=steamos" | "ID=\"steamos\""))
    })
}

/// The engine program for this platform: Steam's Chromium on SteamOS, WebKitGTK on any other
/// Linux, on the Mac WPE WebKit (third_party/wpe, which the app carries: Apple's WebKit is no
/// longer an engine for it), Android's WebView through Spatiand's launcher. `SPATIMESH_ENGINE`
/// names another (`fake`, `webkitgtk`, `cef`, `wpe`, `launcher:<abstract socket>`, or a path).
pub fn program() -> Program {
    let wanted = std::env::var("SPATIMESH_ENGINE").unwrap_or_default();
    #[cfg(target_os = "android")]
    if wanted.is_empty() {
        return Program::Launched(launcher::name());
    }
    let name = match wanted.as_str() {
        "fake" => return Program::Fake("asked for with SPATIMESH_ENGINE".into()),
        "" if cfg!(target_os = "macos") => "spatimesh-wpe",
        "" if is_steamos() => "spatimesh-cef",
        "" => "spatimesh-webkitgtk",
        "webkitgtk" => "spatimesh-webkitgtk",
        "cef" => "spatimesh-cef",
        "wpe" => "spatimesh-wpe",
        // Android's way, with a launcher of one's own listening on this abstract socket.
        #[cfg(any(target_os = "linux", target_os = "android"))]
        launcher if launcher.starts_with("launcher:") => return Program::Launched(launcher["launcher:".len()..].to_string()),
        path => return Program::Path(PathBuf::from(path)),
    };
    match beside(name) {
        Some(path) => Program::Path(path),
        None => Program::Fake(format!("{name} is not installed beside SpatiMesh")),
    }
}

/// How the engine `program` gives extensions their addresses: Chromium (Steam's) keeps
/// chrome-extension:// for its own extension system, Android's WebView gives only the web's
/// schemes an origin (docs/spatimesh.md, "Extensions").
pub fn extension_addresses(program: &Program) -> spatimesh::extensions::set::Addresses {
    use spatimesh::extensions::set::Addresses;
    match program {
        Program::Path(path) if path.file_name().is_some_and(|n| n.to_string_lossy().contains("spatimesh-cef")) => Addresses::Chromium,
        #[cfg(any(target_os = "linux", target_os = "android"))]
        Program::Launched(_) => Addresses::Https,
        _ => Addresses::Browsers,
    }
}

/// One engine host.
pub struct Host {
    child: Option<Child>,
    pub link: Link,
    /// What the engine said it is, once it has.
    pub engine: Option<String>,
    pub pictures: HashMap<TabId, Pictures>,
    /// Why the engine could not be started, if it could not.
    pub failed: Option<String>,
    /// A launched engine, until it has called.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    calling: Option<launcher::Calling>,
    /// The launcher's connection, held while the engine is wanted: its end is the engine's.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    _launched: Option<std::os::unix::net::UnixStream>,
}

impl Host {
    /// Start a host for `profile`.
    pub fn start(program: &Program, store: &Store, profile: &str) -> Result<Host, String> {
        let private = profile == PRIVATE;
        let setup = ProfileSetup {
            id: profile.to_string(),
            private,
            data: if private { PathBuf::new() } else { store.data_dir(profile) },
            cache: if private { PathBuf::new() } else { store.cache_dir(profile) },
        };
        #[cfg(any(target_os = "linux", target_os = "android"))]
        let mut calling = None;
        let (child, link) = match program {
            Program::Path(path) => {
                let (child, link) = spatimesh::host::start(path, &setup).map_err(|e| format!("{}: {e}", path.display()))?;
                (Some(child), link)
            }
            #[cfg(any(target_os = "linux", target_os = "android"))]
            Program::Launched(name) => {
                let (link, call) = launcher::call(name, &setup).map_err(|e| e.to_string())?;
                calling = Some(call);
                (None, link)
            }
            Program::Fake(why) => {
                log::warn!("no web engine ({why}): pages are flat colours");
                let (link, theirs) = Link::pair().map_err(|e| e.to_string())?;
                std::thread::Builder::new()
                    .name("engine".into())
                    .spawn(move || spatimesh::host::serve(Box::new(Fake::new()), Link::from_fd(theirs)))
                    .map_err(|e| e.to_string())?;
                (None, link)
            }
        };
        Ok(Host {
            child,
            link,
            engine: None,
            pictures: HashMap::new(),
            failed: None,
            #[cfg(any(target_os = "linux", target_os = "android"))]
            calling,
            #[cfg(any(target_os = "linux", target_os = "android"))]
            _launched: None,
        })
    }

    pub fn send(&mut self, message: &spatimesh::wire::ToHost) -> bool {
        self.link.send(&message.encode(), &[]).is_ok()
    }

    /// What the engine has said: nothing while a launched engine has not yet called.
    pub fn receive(&mut self) -> Vec<(FromHost, Vec<OwnedFd>)> {
        #[cfg(any(target_os = "linux", target_os = "android"))]
        if let Some(calling) = self.calling.take() {
            match calling.answer() {
                launcher::Answer::Waiting(calling) => {
                    self.calling = Some(calling);
                    return Vec::new();
                }
                launcher::Answer::Failed(why) => {
                    self.failed = Some(why);
                    return Vec::new();
                }
                launcher::Answer::Called(engine, launched, placeholder) => {
                    if let Err(e) = launcher::put_in_place(engine, placeholder, &self.link) {
                        self.failed = Some(e.to_string());
                        return Vec::new();
                    }
                    self._launched = Some(launched);
                }
            }
        }
        self.link.receive_from_host()
    }

    /// Whether the process is still there.
    pub fn alive(&mut self) -> bool {
        if self.failed.is_some() || self.link.is_closed() {
            return false;
        }
        match self.child.as_mut() {
            Some(child) => matches!(child.try_wait(), Ok(None)),
            None => true,
        }
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        let _ = self.link.send(&spatimesh::wire::ToHost::Quit.encode(), &[]);
        if let Some(mut child) = self.child.take() {
            // Given a moment to go by itself (its pages' processes with it), then made to.
            std::thread::spawn(move || {
                for _ in 0..40 {
                    if let Ok(Some(_)) = child.try_wait() {
                        return;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                let _ = child.kill();
                let _ = child.wait();
            });
        }
    }
}

/// Engines a launcher starts, on Android: asked for on the launcher's socket, they call back
/// on one of the window's own. Abstract sockets, which Linux has as well, where this is tested.
#[cfg(any(target_os = "linux", target_os = "android"))]
mod launcher {
    use std::io::{Read, Write};
    #[cfg(target_os = "android")]
    use std::os::android::net::SocketAddrExt;
    use std::os::fd::{AsRawFd, OwnedFd, RawFd};
    #[cfg(target_os = "linux")]
    use std::os::linux::net::SocketAddrExt;
    use std::os::unix::net::{SocketAddr, UnixListener, UnixStream};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use spatimesh::engine::ProfileSetup;
    use spatimesh::wire::Link;

    /// The launcher's socket for this user (MeshLauncher.java).
    #[cfg(target_os = "android")]
    pub fn name() -> String {
        format!("spatimesh-launch-{}", unsafe { libc::getuid() })
    }

    /// How long an engine is given to call, its process started from nothing.
    const CALL_WITHIN: Duration = Duration::from_secs(20);

    /// An engine being started, by a thread of its own so that the window goes on meanwhile.
    pub struct Calling {
        result: mpsc::Receiver<Result<(OwnedFd, UnixStream), String>>,
        /// The other end of what the window has been writing to and watching until the engine
        /// calls.
        placeholder: OwnedFd,
    }

    pub enum Answer {
        Waiting(Calling),
        Failed(String),
        /// The engine's socket, the launcher's connection, and the placeholder.
        Called(OwnedFd, UnixStream, OwnedFd),
    }

    impl Calling {
        pub fn answer(self) -> Answer {
            match self.result.try_recv() {
                Ok(Ok((engine, launched))) => Answer::Called(engine, launched, self.placeholder),
                Ok(Err(why)) => Answer::Failed(why),
                Err(mpsc::TryRecvError::Empty) => Answer::Waiting(self),
                Err(mpsc::TryRecvError::Disconnected) => Answer::Failed("the engine's start ended without a word".into()),
            }
        }
    }

    /// Ask the launcher called `launcher` for an engine for `setup`. The window gets a link at
    /// once: written to and watched meanwhile, it is the engine's socket once the engine calls.
    pub fn call(launcher: &str, setup: &ProfileSetup) -> std::io::Result<(Link, Calling)> {
        let (link, placeholder) = Link::pair()?;
        let wake = placeholder.try_clone()?;
        let (tx, result) = mpsc::channel();
        let (launcher, profile, private) = (launcher.to_string(), setup.id.clone(), setup.private);
        std::thread::Builder::new().name("engine-call".into()).spawn(move || {
            let _ = tx.send(ask(&launcher, &profile, private));
            // Wakes the window: a byte where it is watching, never read.
            unsafe { libc::send(wake.as_raw_fd(), [0u8].as_ptr() as *const _, 1, libc::MSG_NOSIGNAL) };
        })?;
        Ok((link, Calling { result, placeholder }))
    }

    fn peer_uid(fd: RawFd) -> Option<u32> {
        let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        let r = unsafe { libc::getsockopt(fd, libc::SOL_SOCKET, libc::SO_PEERCRED, &mut cred as *mut _ as *mut _, &mut len) };
        (r == 0).then_some(cred.uid)
    }

    fn ask(launcher: &str, profile: &str, private: bool) -> Result<(OwnedFd, UnixStream), String> {
        if profile.is_empty() || profile.contains(char::is_whitespace) {
            return Err(format!("not a profile's name: {profile:?}"));
        }
        let uid = unsafe { libc::getuid() };
        let name = format!("spatimesh-{}-{}", std::process::id(), spatimesh::store::new_id());
        let address = |name: &str| SocketAddr::from_abstract_name(name.as_bytes()).map_err(|e| e.to_string());
        let listener = UnixListener::bind_addr(&address(&name)?).map_err(|e| format!("no socket for the engine: {e}"))?;
        let mut asked = UnixStream::connect_addr(&address(launcher)?)
            .map_err(|e| format!("Spatiand's launcher for engines is not there ({e})"))?;
        if peer_uid(asked.as_raw_fd()) != Some(uid) {
            return Err("the launcher's socket is another user's".into());
        }
        asked.set_read_timeout(Some(Duration::from_secs(10))).map_err(|e| e.to_string())?;
        asked
            .write_all(format!("start {profile} {} {name}\n", private as u8).as_bytes())
            .map_err(|e| format!("the launcher did not listen: {e}"))?;
        let mut reply = Vec::new();
        let mut byte = [0u8; 1];
        while reply.len() < 4096 {
            match asked.read(&mut byte) {
                Ok(1) if byte[0] == b'\n' => break,
                Ok(1) => reply.push(byte[0]),
                Ok(_) => return Err("the launcher hung up".into()),
                Err(e) => return Err(format!("the launcher did not answer: {e}")),
            }
        }
        match String::from_utf8_lossy(&reply).trim() {
            "ok" => {}
            "full" => return Err("four profiles' pages are open already; close one's tabs first".into()),
            other => return Err(other.strip_prefix("no ").unwrap_or(other).to_string()),
        }
        // The engine's call, from this user only.
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let until = Instant::now() + CALL_WITHIN;
        loop {
            match listener.accept() {
                Ok((stream, _)) if peer_uid(stream.as_raw_fd()) == Some(uid) => {
                    stream.set_nonblocking(false).map_err(|e| e.to_string())?;
                    asked.set_read_timeout(None).map_err(|e| e.to_string())?;
                    return Ok((stream.into(), asked));
                }
                Ok(_) => log::warn!("a call to SpatiMesh from another user, refused"),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() > until {
                        return Err(format!("the engine did not call within {} seconds", CALL_WITHIN.as_secs()));
                    }
                    let mut p = libc::pollfd { fd: listener.as_raw_fd(), events: libc::POLLIN, revents: 0 };
                    unsafe { libc::poll(&mut p, 1, 100) };
                }
                Err(e) => return Err(format!("the engine's call failed: {e}")),
            }
        }
    }

    /// The engine's socket put where the window has been writing and watching: what it wrote
    /// meanwhile is passed on first, then the engine's socket takes the placeholder's number,
    /// so that the window, which only ever sees that number, carries on.
    pub fn put_in_place(engine: OwnedFd, placeholder: OwnedFd, link: &Link) -> std::io::Result<()> {
        let mut engine = UnixStream::from(engine);
        let mut written = UnixStream::from(placeholder);
        written.set_nonblocking(true)?;
        let mut buffer = [0u8; 65536];
        loop {
            match written.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => engine.write_all(&buffer[..n])?,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) => return Err(e),
            }
        }
        let fd = link.raw_fd();
        // SAFETY: `fd` stays open throughout, now on the engine's socket; Link sets every socket
        // it reads to not block, which the new one is made too, and like it not inherited.
        unsafe {
            if libc::dup2(engine.as_raw_fd(), fd) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
            libc::fcntl(fd, libc::F_SETFL, libc::fcntl(fd, libc::F_GETFL) | libc::O_NONBLOCK);
        }
        Ok(())
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use std::io::{Read, Write};
    use std::os::fd::OwnedFd;
    use std::os::linux::net::SocketAddrExt;
    use std::os::unix::net::{SocketAddr, UnixListener, UnixStream};
    use std::time::{Duration, Instant};

    use spatimesh::engine::fake::Fake;
    use spatimesh::engine::View;
    use spatimesh::store::Store;
    use spatimesh::wire::{FromHost, Link, ToHost};

    use super::{Host, Program};

    fn read_line(stream: &mut UnixStream) -> String {
        let mut line = Vec::new();
        let mut byte = [0u8; 1];
        while stream.read(&mut byte).unwrap() == 1 && byte[0] != b'\n' {
            line.push(byte[0]);
        }
        String::from_utf8(line).unwrap()
    }

    /// Until the host has said Hello and given a tab's buffers, or has failed.
    fn hear(host: &mut Host) -> (Option<String>, usize) {
        let (mut hello, mut fds) = (None, 0);
        let until = Instant::now() + Duration::from_secs(20);
        while Instant::now() < until && (hello.is_none() || fds == 0) && host.alive() {
            host.link.wait_readable(100);
            for (message, with) in host.receive() {
                match message {
                    FromHost::Hello { engine } => hello = Some(engine),
                    FromHost::Buffers { .. } => fds = with.len(),
                    _ => {}
                }
            }
        }
        (hello, fds)
    }

    /// Android's launcher, as MeshLauncher.java does it, with the engine that needs none.
    #[test]
    fn an_engine_a_launcher_started() {
        let name = format!("spatimesh-test-launcher-{}", spatimesh::store::new_id());
        let launcher = UnixListener::bind_addr(&SocketAddr::from_abstract_name(name.as_bytes()).unwrap()).unwrap();
        let launched = std::thread::spawn(move || {
            let (mut asked, _) = launcher.accept().unwrap();
            let line = read_line(&mut asked);
            let words: Vec<&str> = line.split(' ').collect();
            assert_eq!(&words[..3], &["start", "default", "0"], "{line}");
            asked.write_all(b"ok\n").unwrap();
            let engine = UnixStream::connect_addr(&SocketAddr::from_abstract_name(words[3].as_bytes()).unwrap()).unwrap();
            let engine: OwnedFd = engine.into();
            std::thread::spawn(move || spatimesh::host::serve(Box::new(Fake::new()), Link::from_fd(engine)));
            // Held while the window wants the engine; its end is the engine's.
            let mut rest = Vec::new();
            let _ = asked.read_to_end(&mut rest);
        });
        let root = std::env::temp_dir().join(format!("spatimesh-launch-{}", spatimesh::store::new_id()));
        let mut host = Host::start(&Program::Launched(name), &Store::at(&root), "default").unwrap();
        // Said before the engine has called: kept for it.
        assert!(host.send(&ToHost::Open { tab: 7, url: "https://example.com/".into(), view: View::new(64, 48, 1.0) }));
        let (hello, fds) = hear(&mut host);
        assert!(host.failed.is_none(), "{:?}", host.failed);
        assert_eq!(hello.as_deref(), Some("no engine (flat pages)"));
        assert_eq!(fds, 2, "the tab opened before the call has its pictures");
        drop(host);
        launched.join().unwrap();
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn no_launcher() {
        let name = format!("spatimesh-test-nobody-{}", spatimesh::store::new_id());
        let root = std::env::temp_dir().join(format!("spatimesh-launch-{}", spatimesh::store::new_id()));
        let mut host = Host::start(&Program::Launched(name), &Store::at(&root), "default").unwrap();
        let (hello, _) = hear(&mut host);
        assert!(hello.is_none());
        assert!(!host.alive());
        assert!(host.failed.as_deref().is_some_and(|why| why.contains("launcher")), "{:?}", host.failed);
    }

    #[test]
    fn a_full_launcher() {
        let name = format!("spatimesh-test-full-{}", spatimesh::store::new_id());
        let launcher = UnixListener::bind_addr(&SocketAddr::from_abstract_name(name.as_bytes()).unwrap()).unwrap();
        std::thread::spawn(move || {
            let (mut asked, _) = launcher.accept().unwrap();
            read_line(&mut asked);
            asked.write_all(b"full\n").unwrap();
        });
        let root = std::env::temp_dir().join(format!("spatimesh-launch-{}", spatimesh::store::new_id()));
        let mut host = Host::start(&Program::Launched(name), &Store::at(&root), "work").unwrap();
        hear(&mut host);
        assert!(host.failed.as_deref().is_some_and(|why| why.contains("four profiles")), "{:?}", host.failed);
    }
}
