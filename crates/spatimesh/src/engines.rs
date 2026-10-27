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

/// Whether Spatiand's own WPE is installed beside SpatiMesh: on Linux the bundle
/// third_party/wpe/build.sh makes, in `wpe/` (a `spatimesh-wpe` built without it is in every
/// build folder); on Android its processes, among the app's libraries.
fn wpe_beside() -> bool {
    let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.to_path_buf())) else { return false };
    if cfg!(target_os = "android") {
        dir.join("libWPEWebProcess.so").is_file()
    } else {
        dir.join("wpe/lib").is_dir()
    }
}

/// The engine program for this platform: WPE WebKit, which Spatiand carries (third_party/wpe),
/// where it is installed; on a Linux desktop without it, the distro's WebKitGTK. Android has
/// none without WPE (Android's WebView and Steam's Chromium are no longer engines).
/// `SPATIMESH_ENGINE` names another (`fake`, `webkitgtk`, `wpe`, or a path).
pub fn program() -> Program {
    let wanted = std::env::var("SPATIMESH_ENGINE").unwrap_or_default();
    let name = match wanted.as_str() {
        "fake" => return Program::Fake("asked for with SPATIMESH_ENGINE".into()),
        "" if cfg!(target_os = "macos") => "spatimesh-wpe",
        "" if wpe_beside() => "spatimesh-wpe",
        "" if cfg!(target_os = "android") => return Program::Fake("no engine on Android: WPE is not in the app".into()),
        "" => "spatimesh-webkitgtk",
        "webkitgtk" => "spatimesh-webkitgtk",
        "wpe" => "spatimesh-wpe",
        path => return Program::Path(PathBuf::from(path)),
    };
    match beside(name) {
        Some(path) => Program::Path(path),
        None => Program::Fake(format!("{name} is not installed beside SpatiMesh")),
    }
}

/// How the engine gives extensions their addresses: every engine now as browsers do
/// (docs/spatimesh.md, "Extensions").
pub fn extension_addresses(_program: &Program) -> spatimesh::extensions::set::Addresses {
    spatimesh::extensions::set::Addresses::Browsers
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
        let (child, link) = match program {
            Program::Path(path) => {
                let (child, link) = spatimesh::host::start(path, &setup).map_err(|e| format!("{}: {e}", path.display()))?;
                (Some(child), link)
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
        })
    }

    pub fn send(&mut self, message: &spatimesh::wire::ToHost) -> bool {
        self.link.send(&message.encode(), &[]).is_ok()
    }

    /// What the engine has said.
    pub fn receive(&mut self) -> Vec<(FromHost, Vec<OwnedFd>)> {
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
