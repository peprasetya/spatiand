//! A session with nothing to show it on.
//!
//! Everything a session is apart from its pictures: the Wayland and X11 displays, the seat,
//! the hosts and their windows, the clipboard, and the control socket that lets a test drive
//! them. Nothing is drawn and no headset is opened, so it runs on a Deck in desktop mode with
//! the glasses in a drawer, over `ssh`, with no screen involved at all.
//!
//! It exists for the tests that have to go through the real compositor to mean anything --
//! copying in one application and pasting in another, on this machine and on a host -- and
//! it is `SPATIAND_BACKEND=headless`. Like the host, it wants only a render node: clients
//! still need dmabuf offered to them, and pictures arriving from a host still need somewhere
//! to go, even if nobody looks at them.
//!
//! **A head that is not there.** `SPATIAND_HEADLESS_HEAD=<degrees a second>` gives the session
//! poses to hand out, where it otherwise says there are none: a head turning on the spot at
//! that rate (0 holds it still), written into the pose channel each turn of the loop as the
//! Deck's loop writes the glasses', each numbered -- the slot's `reserved`, as a host numbers
//! the viewports it is sent -- so that a test of an application that draws from the head can
//! see poses arrive and change, and see the number its picture is handed back with (the
//! control socket's `frame`). Nothing is opened for it and nothing is drawn.

use std::time::Duration;

use smithay::backend::allocator::gbm::GbmDevice;
use smithay::backend::egl::{EGLContext, EGLDisplay};
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
use smithay::utils::DeviceFd;

use spatiand_shell::{DesktopPanels, Shell};

use crate::{Runtime, Spatiand};

pub fn run(
    event_loop: &mut EventLoop<'static, Runtime>,
    display: &mut Display<Spatiand>,
    runtime: &mut Runtime,
) -> Result<(), Box<dyn std::error::Error>> {
    let node =
        std::env::var("SPATIAND_RENDER_NODE").unwrap_or_else(|_| "/dev/dri/renderD128".into());
    let file = std::fs::OpenOptions::new().read(true).write(true).open(&node)?;
    let gbm = GbmDevice::new(DeviceFd::from(std::os::fd::OwnedFd::from(file)))?;
    // SAFETY: the device outlives the display and context, which live to the end of `run`.
    let egl_display = unsafe { EGLDisplay::new(gbm)? };
    let egl_context = EGLContext::new(&egl_display)?;
    // SAFETY: as above; the context is used on this thread only.
    let mut renderer = unsafe { GlesRenderer::new(egl_context)? };
    crate::dmabuf::advertise(&mut runtime.state, &renderer);
    log::info!("headless: renderer on {node}, nothing will be drawn");

    let x_display = crate::xwayland::start(&runtime.display_handle, &event_loop.handle());
    log::info!(
        "headless: run a client with  WAYLAND_DISPLAY={}{}",
        runtime.state.socket_name,
        x_display.map_or(String::new(), |n| format!("  DISPLAY=:{n}"))
    );

    // A shell nobody sees, because the hosts report what they serve to one. Applications on a
    // host are started from the host's own control socket, as its settings app does.
    let mut shell = Shell::new(Vec::new(), DesktopPanels::NONE, true);
    shell.set_local_name(&crate::system::machine_name());
    let mut prefs = crate::prefs::Prefs::load();
    let mut remotes = crate::remote::Remotes::start(&mut runtime.display_handle, &prefs);
    let control = crate::control::Control::start();

    // The head that is not there: see the top of this file.
    let head_turn = std::env::var("SPATIAND_HEADLESS_HEAD").ok().and_then(|v| v.parse::<f64>().ok());
    if let Some(rate) = head_turn {
        log::info!("headless: handing out the poses of a head turning {rate} degrees a second");
    }
    let head_started = std::time::Instant::now();
    let stereo = spatiand_render::StereoConfig::default();
    let mut pose_channel: Option<crate::pose::Channel> = None;
    let mut pose_waiting = crate::pose::Waiting::default();
    let mut poses_written: i64 = 0;

    while runtime.state.running {
        // SIGTERM is caught for every backend (`main` installs the handler), so a loop that does
        // not look is a process `systemctl stop` has to wait out and then kill.
        if crate::shutdown::requested() {
            log::info!("headless: asked to stop");
            break;
        }
        remotes.tick(&mut runtime.display_handle, &mut prefs, &mut shell);
        if let Some(control) = &control {
            control.serve(&mut runtime.state);
        }
        crate::clipboard::exchange(&mut remotes, &mut runtime.state);
        // Every dmabuf a client offers waits here to be answered, and holds its descriptors
        // until it is. Leaving this out held every picture a host sent for the life of the
        // session: a test run of a few hundred pastes reached the descriptor limit and took
        // the link to the host down with it.
        crate::dmabuf::settle(&mut runtime.state, &mut renderer);

        let screen = runtime.state.screen.clone();
        runtime.state.send_frames(&screen, crate::pose::frame_time());
        match head_turn {
            Some(rate) => {
                crate::pose::answer(&mut runtime.state, true, &mut pose_channel, &mut pose_waiting);
                if let Some(channel) = pose_channel.as_mut() {
                    // About the vertical, which is +Z in the room's own frame.
                    let yaw = (rate * head_started.elapsed().as_secs_f64()).to_radians();
                    let sample = crate::pose::now_ns();
                    let mut slot = crate::pose::wire_slot(
                        glam::DQuat::from_rotation_z(yaw),
                        glam::DVec3::ZERO,
                        &stereo,
                        sample,
                        sample + 14_000_000,
                    );
                    poses_written += 1;
                    slot.reserved = poses_written;
                    channel.write_slot(slot);
                }
            }
            None => crate::pose::refuse(&mut runtime.state),
        }
        runtime.state.space.refresh();
        runtime.state.settle_keyboard_focus();
        display.dispatch_clients(&mut runtime.state)?;
        display.flush_clients()?;
        event_loop.dispatch(Some(Duration::from_millis(14)), runtime)?;
    }
    drop(renderer);
    Ok(())
}
