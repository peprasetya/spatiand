//! The engine host: one profile's web engine, spoken to by the window over a [`Link`].
//!
//! Each engine is a program of its own (`spatimesh-webkitgtk`, `spatimesh-cef`,
//! `spatimesh-webkit`), so that the window process links none of them and a system without one
//! engine's libraries can still run the others. Each program's `main` is [`main`] with its
//! engine: what to do with the window's messages is the same whatever the engine.

use std::os::fd::{FromRawFd, OwnedFd};
use std::path::PathBuf;
use std::time::Duration;

use crate::engine::{Engine, Happened, ProfileSetup};
use crate::wire::{FromHost, Link, SharedCanvas, ToHost};

/// How the window tells a host where its end of the socket is.
pub const FD_VARIABLE: &str = "SPATIMESH_FD";

/// Start the engine program `program` for a profile, with a socket to it: what the window
/// does for each profile in use, and what tests do.
pub fn start(program: &std::path::Path, setup: &ProfileSetup) -> std::io::Result<(std::process::Child, Link)> {
    use std::os::fd::AsRawFd;
    use std::os::unix::process::CommandExt;
    let (link, theirs) = Link::pair()?;
    let raw = theirs.as_raw_fd();
    let mut command = std::process::Command::new(program);
    command.args(arguments(setup)).env(FD_VARIABLE, "3");
    // SAFETY: between fork and exec, only async-signal-safe calls: dup2 and fcntl.
    unsafe {
        command.pre_exec(move || {
            if raw == 3 {
                // Already where it goes; dup2 would leave it closed on exec.
                if libc::fcntl(3, libc::F_SETFD, 0) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
            } else if libc::dup2(raw, 3) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = command.spawn()?;
    drop(theirs);
    Ok((child, link))
}

/// The arguments a window starts a host with, for `profile`.
pub fn arguments(setup: &ProfileSetup) -> Vec<String> {
    let mut args = vec!["--profile".to_string(), setup.id.clone()];
    if setup.private {
        args.push("--private".to_string());
    } else {
        args.extend(["--data".to_string(), setup.data.display().to_string(), "--cache".to_string(), setup.cache.display().to_string()]);
    }
    args
}

/// The profile a host was started for, read back from [`arguments`].
pub fn setup_from(args: &[String]) -> Result<ProfileSetup, String> {
    let mut setup = ProfileSetup { id: String::new(), private: false, data: PathBuf::new(), cache: PathBuf::new() };
    let mut i = 0;
    while i < args.len() {
        let value = || args.get(i + 1).cloned().ok_or(format!("{} wants a value", args[i]));
        match args[i].as_str() {
            "--profile" => setup.id = value()?,
            "--private" => {
                setup.private = true;
                i -= 1;
            }
            "--data" => setup.data = PathBuf::from(value()?),
            "--cache" => setup.cache = PathBuf::from(value()?),
            other => return Err(format!("not an engine host's argument: {other}")),
        }
        i += 2;
    }
    if setup.id.is_empty() {
        return Err("no --profile".into());
    }
    if !setup.private && (setup.data.as_os_str().is_empty() || setup.cache.as_os_str().is_empty()) {
        return Err("a profile that keeps things wants --data and --cache".into());
    }
    Ok(setup)
}

/// An engine program's `main`: read the profile from the arguments, make the engine, serve the
/// window that started this process until it goes.
pub fn main(make: impl FnOnce(&ProfileSetup) -> Result<Box<dyn Engine>, String>) {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let setup = match setup_from(&args) {
        Ok(setup) => setup,
        Err(e) => {
            eprintln!("spatimesh engine: {e}");
            std::process::exit(2);
        }
    };
    let Some(fd) = std::env::var(FD_VARIABLE).ok().and_then(|v| v.parse::<i32>().ok()) else {
        eprintln!("spatimesh engine: started by something that is not SpatiMesh (no {FD_VARIABLE})");
        std::process::exit(2);
    };
    // SAFETY: the window put its end of the socket at this number for us, and nothing else here
    // has it.
    let link = Link::from_fd(unsafe { OwnedFd::from_raw_fd(fd) });
    let engine = match make(&setup) {
        Ok(engine) => engine,
        Err(e) => {
            log::error!("the engine would not start: {e}");
            let mut link = link;
            let _ = link.send(&FromHost::Hello { engine: format!("failed: {e}") }.encode(), &[]);
            std::process::exit(1);
        }
    };
    serve(engine, link);
}

/// Serve the window at the other end of `link` with `engine` until it says to stop or goes.
pub fn serve(mut engine: Box<dyn Engine>, mut link: Link) {
    let mut canvas = SharedCanvas::new();
    engine.wake_on(link.raw_fd());
    if link.send(&FromHost::Hello { engine: engine.name() }.encode(), &[]).is_err() {
        return;
    }
    loop {
        for message in link.receive_to_host() {
            match message {
                ToHost::Quit => return,
                ToHost::Open { tab, url, view } => {
                    if let Err(e) = engine.open(tab, &url, view) {
                        let failed = Happened::Crashed { tab, why: e };
                        if link.send(&FromHost::Happened(failed).encode(), &[]).is_err() {
                            return;
                        }
                    }
                }
                ToHost::Close { tab } => {
                    engine.close(tab);
                    canvas.forget(tab);
                }
                ToHost::Load { tab, url } => engine.load(tab, &url),
                ToHost::Back { tab } => engine.back(tab),
                ToHost::Forward { tab } => engine.forward(tab),
                ToHost::Reload { tab } => engine.reload(tab),
                ToHost::Stop { tab } => engine.stop(tab),
                ToHost::Resize { tab, view } => engine.resize(tab, view),
                ToHost::Pointer { tab, x, y, button, modifiers } => engine.pointer(tab, x, y, button, modifiers),
                ToHost::PointerLeft { tab } => engine.pointer_left(tab),
                ToHost::Scroll { tab, x, y, dx, dy, modifiers } => engine.scroll(tab, x, y, dx, dy, modifiers),
                ToHost::Key { tab, key } => engine.key(tab, &key),
                ToHost::Focus { tab, focused } => engine.focus(tab, focused),
                ToHost::Window { tab, window } => engine.set_window(tab, window),
                ToHost::Mute { tab, muted } => engine.set_muted(tab, muted),
                ToHost::Release { tab, buffer } => canvas.release(tab, buffer),
                ToHost::Choose { tab, index } => engine.choose(tab, index),
                ToHost::Answer { tab, accepted, text } => engine.answer(tab, accepted, text),
                ToHost::Extensions { set } => match serde_json::from_str(&set) {
                    Ok(set) => engine.set_extensions(&set),
                    Err(e) => log::warn!("the profile's extensions could not be read: {e}"),
                },
                ToHost::RunScript { tab, world, all_frames, code, request, boot } => engine.run_script(tab, &world, all_frames, &code, request, boot),
            }
        }
        if link.is_closed() {
            // The window went without saying: it crashed, or was killed. Nothing to draw for.
            return;
        }
        let happened = engine.pump(Duration::from_millis(50), &mut canvas);
        for h in happened {
            // A picture is told by the canvas, with its buffer.
            if matches!(h, Happened::Picture { .. }) {
                continue;
            }
            if link.send(&FromHost::Happened(h).encode(), &[]).is_err() {
                return;
            }
        }
        for (message, fds) in canvas.take_outbox() {
            if link.send(&message.encode(), &fds).is_err() {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_come_back() {
        let kept = ProfileSetup { id: "default".into(), private: false, data: "/d/default".into(), cache: "/c/default".into() };
        assert_eq!(setup_from(&arguments(&kept)), Ok(kept));
        let private = ProfileSetup { id: "private".into(), private: true, data: PathBuf::new(), cache: PathBuf::new() };
        assert_eq!(setup_from(&arguments(&private)), Ok(private));
        assert!(setup_from(&["--profile".into()]).is_err());
        assert!(setup_from(&["--profile".into(), "x".into()]).is_err(), "a kept profile without its folders");
        assert!(setup_from(&["--nonsense".into()]).is_err());
    }
}
