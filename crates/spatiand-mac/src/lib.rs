//! Spatiand on the Mac, as a library inside `Spatiand.app`.
//!
//! **This is the Deck's compositor, not a copy of it** -- built the way `spatiand-android` is.
//! Every module below that is not in `mac/` is `crates/spatiand/src`'s own file, compiled here
//! by path: the same windows with the same frames, the same HUD and launcher, the same pointer,
//! keyboard and menus. What is the Mac's is only what the Deck gets from its hardware:
//!
//! * the picture goes to the glasses through ANGLE -- OpenGL ES on Metal -- into a layer the app
//!   hands over, instead of DRM ([`mac::egl`], [`mac::backend`]);
//! * a window of this Mac's own applications is captured by the app and arrives as an
//!   `IOSurface`; a host's window is decoded by VideoToolbox into one. Both are drawn from
//!   there ([`mac::remote_video`]), and both are ordinary windows of the compositor;
//! * the Mac's mouse, trackpad, keyboard and game controllers are made into the input the Deck's
//!   session reads.
//!
//! Anywhere but macOS this library is empty.

#![cfg(target_os = "macos")]
// The Deck's modules carry what the DRM backend reads and this one does not.
#![allow(dead_code, unused_imports)]

pub mod mac;

/// The app's link to a host when there are no glasses, and this Mac as a host: its C interface
/// is part of this library's.
pub use spatiand_mac_core as link;

/// The two helpers of the Deck's backend that the snapshot names. The backend itself is DRM
/// from end to end and is not built here.
mod backend_drm {
    pub use crate::mac::panel::{fit_panel, head_locked_panel_sized};
}

#[path = "../../spatiand/src/attention.rs"]
mod attention;
#[path = "../../spatiand/src/audio.rs"]
mod audio;
#[path = "../../spatiand/src/backend_snapshot.rs"]
mod backend_snapshot;
#[path = "../../spatiand/src/bluetooth.rs"]
mod bluetooth;
#[path = "../../spatiand/src/cadence.rs"]
mod cadence;
#[path = "../../spatiand/src/calib.rs"]
mod calib;
#[path = "../../spatiand/src/click.rs"]
mod click;
#[path = "../../spatiand/src/clipboard.rs"]
mod clipboard;
#[path = "../../spatiand/src/control.rs"]
mod control;
#[path = "../../spatiand/src/controls.rs"]
mod controls;
#[path = "../../spatiand/src/diagram.rs"]
mod diagram;
#[path = "../../spatiand/src/dmabuf.rs"]
mod dmabuf;
#[path = "../../spatiand/src/environment.rs"]
mod environment;
#[path = "../../spatiand/src/gl.rs"]
mod gl;
#[path = "../../spatiand/src/icon.rs"]
mod icon;
#[path = "../../spatiand/src/imu_record.rs"]
mod imu_record;
#[path = "../../spatiand/src/input_map.rs"]
mod input_map;
#[path = "../../spatiand/src/keyboard_face.rs"]
mod keyboard_face;
#[path = "../../spatiand/src/menu.rs"]
mod menu;
#[path = "../../spatiand/src/pip.rs"]
mod pip;
#[path = "../../spatiand/src/pointer.rs"]
mod pointer;
#[path = "../../spatiand/src/pose.rs"]
mod pose;
#[path = "../../spatiand/src/prefs.rs"]
mod prefs;
#[path = "../../spatiand/src/remote/mod.rs"]
mod remote;
#[path = "../../spatiand/src/scene.rs"]
mod scene;
#[path = "../../spatiand/src/shutdown.rs"]
mod shutdown;
#[path = "../../spatiand/src/sidecar.rs"]
mod sidecar;
#[path = "../../spatiand/src/startup.rs"]
mod startup;
#[path = "../../spatiand/src/state.rs"]
mod state;
#[path = "../../spatiand/src/status.rs"]
mod status;
#[path = "../../spatiand/src/system.rs"]
mod system;
#[path = "../../spatiand/src/volume_keys.rs"]
mod volume_keys;
#[path = "../../spatiand/src/waiting.rs"]
mod waiting;
#[path = "../../spatiand/src/window.rs"]
mod window;
#[path = "../../spatiand/src/xr.rs"]
mod xr;
#[path = "../../spatiand/src/xwayland.rs"]
mod xwayland;

use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::{Display, DisplayHandle};

pub use state::Spatiand;

/// What the event loop carries: `main.rs`'s, for the modules that name it.
pub struct Runtime {
    pub state: Spatiand,
    pub display_handle: DisplayHandle,
}

/// One frame to a PNG, as the Deck's `SPATIAND_BACKEND=snapshot` does and with its settings:
/// see `backend_snapshot.rs`.
pub fn snapshot() -> Result<(), Box<dyn std::error::Error>> {
    mac::prepare();
    let mut event_loop: EventLoop<Runtime> = EventLoop::try_new()?;
    let mut display: Display<Spatiand> = Display::new()?;
    let display_handle = display.handle();
    let state = Spatiand::new(&mut display, &event_loop.handle());
    let mut runtime = Runtime { state, display_handle };
    backend_snapshot::run(&mut event_loop, &mut display, &mut runtime)
}
