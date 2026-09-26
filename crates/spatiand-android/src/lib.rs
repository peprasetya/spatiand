//! Spatiand on Android, as `libspatiand.so` inside the `id.prasetya.spatiand` app.
//!
//! **This is the Deck's compositor, not a copy of it.** Every module below that is not in
//! `android/` is `crates/spatiand/src`'s own file, compiled here by path: the same windows with
//! the same frames, the same HUD and launcher, the same pointer, keyboard and menus. What is
//! Android's is only what the Deck gets from its hardware:
//!
//! * the picture goes to the glasses' display through an `ANativeWindow` the app hands over,
//!   instead of DRM ([`android::backend`]);
//! * the controller is the phone -- its screen is the left pad, its orientation the right,
//!   the orange key STEAM and the button under the touch area `⋯` -- made into the Deck's own
//!   controller state, so everything that reads a Deck reads it unchanged
//!   ([`android::controller`]);
//! * remote windows are decoded by `MediaCodec` into GPU buffers (`spatiand_video::android`),
//!   and drawn from there ([`android::remote_video`]).
//!
//! The app is the Beam Pro's glasses shell in place of XREAL's Nebula: `docs/beam-pro.md`.
//! Anywhere but Android this library is empty, so a workspace build on the Mac or the Deck does
//! not have to know it exists.

#![cfg(target_os = "android")]
// The Deck's modules carry what the DRM backend reads and this one does not.
#![allow(dead_code, unused_imports)]

mod android;

#[path = "../../spatiand/src/attention.rs"]
mod attention;
#[path = "../../spatiand/src/audio.rs"]
mod audio;
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
#[path = "../../spatiand/src/startup.rs"]
mod startup;
#[path = "../../spatiand/src/sidecar.rs"]
mod sidecar;
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

use smithay::reexports::wayland_server::DisplayHandle;

pub use state::Spatiand;

/// What the event loop carries: `main.rs`'s, for the modules that name it.
pub struct Runtime {
    pub state: Spatiand,
    pub display_handle: DisplayHandle,
}
