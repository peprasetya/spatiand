//! The audio server's interface on Android, where there is no PipeWire.
//!
//! Spatiand on the Beam Pro has no local applications to give sinks to: every window there is a
//! remote one, whose sound arrives over the network and is played by the session itself. So the
//! engine here keeps the shape of `server.rs` -- the compositor calls it exactly as it does on
//! the Deck -- and places nothing yet. Placing a remote window's sound at the window is this
//! module's to grow, on AAudio.

use crate::render::Directness;
use crate::stage::{Layout, Speaker};

/// Kept for the shape of the API; nothing on Android reads it.
pub const ROUTING_ENV: &str = "PIPEWIRE_PROPS";
pub const PULSE_ROUTING_ENV: &str = "PULSE_SINK";

/// How wide a window's sink should be. See `server.rs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Width {
    Stereo,
    Full,
}

impl Width {
    pub fn layout(self) -> Layout {
        match self {
            Width::Stereo => Layout::Stereo,
            Width::Full => Layout::Surround714,
        }
    }
}

pub type Slot = u64;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Status {
    pub layout: Option<Layout>,
    pub sounding: Option<Layout>,
    pub peak: f32,
    pub muted: bool,
}

pub const SINK_DESCRIPTION: &str = "Spatiand window";

pub fn sink_name(slot: Slot) -> String {
    format!("spatiand.window.{slot}")
}

pub fn routing_env(slot: Slot) -> Vec<(String, String)> {
    let sink = sink_name(slot);
    vec![
        (ROUTING_ENV.to_string(), format!("{{ target.object = \"{sink}\" }}")),
        (PULSE_ROUTING_ENV.to_string(), sink),
    ]
}

#[derive(Debug, Clone, Copy)]
pub enum Head {
    Measured,
    Reasoned,
}

/// An engine with nowhere to play: every window's sound goes where the session sends it.
pub struct Engine {
    rate: u32,
}

impl Engine {
    pub fn start(rate: u32, _head: Head, _directness: Directness) -> Engine {
        log::info!("spatial audio: not placed on Android yet; remote sound plays unplaced");
        Engine { rate }
    }

    pub fn rate(&self) -> u32 {
        self.rate
    }

    pub fn open(&self, _slot: Slot, _width: Width) {}

    pub fn close(&self, _slot: Slot) {}

    pub fn aim(&self, _slot: Slot, _speakers: Vec<Speaker>, _off_axis: f64) {}

    pub fn set_muted(&self, _slot: Slot, _muted: bool) {}

    pub fn set_directness(&self, _directness: Directness) {}

    pub fn status(&self, _slot: Slot) -> Option<Status> {
        None
    }

    pub fn sounding(&self, _floor: f32) -> Vec<(Slot, Status)> {
        Vec::new()
    }
}
