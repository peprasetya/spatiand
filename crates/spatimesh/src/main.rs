//! SpatiMesh: Spatiand's web browser.
//!
//! A full browser (docs/spatimesh.md), drawn straight into the room, on the platform's own web
//! engine: this is the window process, which holds every SpatiMesh window and draws what is
//! around the page; the pages are drawn by an engine host per profile (`engines.rs`), each
//! tab's page in a process of the engine's own. Starting SpatiMesh again opens a window in the
//! one already running, so that a tab can be dragged between any two of its windows.
//!
//! ```text
//! spatimesh [--private] [--load-extension <folder or package>]... [address]
//! ```

mod browser;
mod engines;
mod page;

use std::io::Write;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;

use spatiand_app::App;

/// Where a running SpatiMesh listens for another start: one per room, by its socket's name.
fn socket_path() -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let display = std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "wayland-0".into());
    let display = display.rsplit('/').next().unwrap_or("wayland").to_string();
    dir.join(format!("spatimesh-{display}.sock"))
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    // Pages' pictures come from the engines in private: before any is started (and after
    // what Spatiand itself said was read, for this window's own pictures).
    spatiand_surfaces::listen();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut private = false;
    let mut url = None;
    // Extensions to install in the first profile before anything opens, as Chrome's
    // --load-extension loads them (they stay installed).
    let mut extensions = Vec::new();
    let mut words = args.iter();
    while let Some(word) = words.next() {
        match word.as_str() {
            "--private" => private = true,
            "--load-extension" => extensions.extend(words.next().cloned()),
            w if w.starts_with("--load-extension=") => extensions.push(w["--load-extension=".len()..].to_string()),
            w if !w.starts_with("--") => url = Some(w.to_string()),
            _ => {}
        }
    }
    // Already running in this room: that SpatiMesh opens the window.
    let path = socket_path();
    if let Ok(mut stream) = UnixStream::connect(&path) {
        let asked = format!("window {} {}", if private { "private" } else { "kept" }, url.clone().unwrap_or_default());
        if stream.write_all(asked.trim_end().as_bytes()).is_ok() {
            log::info!("SpatiMesh is already running; it opens the window");
            return;
        }
    }
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).ok().inspect(|l| {
        let _ = l.set_nonblocking(true);
    });
    let run = || -> Result<(), String> {
        let app = App::connect("id.prasetya.spatiand.spatimesh")?;
        let mut browser = browser::Browser::new(app, listener);
        let first = spatimesh::profiles::list(&spatimesh::store::Store::open())[0].id.clone();
        browser.load_extensions(&first, &extensions);
        let profile = if private { spatimesh::profiles::PRIVATE.to_string() } else { first };
        browser.new_window(&profile, url.as_deref());
        browser.run()
    };
    let ended = run();
    let _ = std::fs::remove_file(&path);
    if let Err(e) = ended {
        log::error!("{e}");
        std::process::exit(1);
    }
}
