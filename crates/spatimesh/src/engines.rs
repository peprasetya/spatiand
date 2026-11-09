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

/// The engine program: none of its own. SpatiMesh is Chromium (third_party/chromium), which the
/// launcher starts in place of this program wherever it is installed (crates/spatiand/src/own.rs);
/// this program, its window and its stores are what Chromium's are kept in step with, and draw
/// pages only through an engine named by `SPATIMESH_ENGINE` (a path, or `fake`). WPE WebKit was
/// the engine here until 2026-11-10.
pub fn program() -> Program {
    match std::env::var("SPATIMESH_ENGINE").unwrap_or_default().as_str() {
        "fake" => Program::Fake("asked for with SPATIMESH_ENGINE".into()),
        "" => Program::Fake("no engine: SpatiMesh is Chromium, which is not installed here (third_party/chromium)".into()),
        path => Program::Path(PathBuf::from(path)),
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
