//! What a film is, and how it is played.
//!
//! The Vibrefy Rust port's `vibrefy-media`, moved into Spatiand to be SpatiReel's engine
//! (docs/spatiand-apps.md). The parts that need neither a GPU nor a decoder are on their own
//! so they can be tested without either: [`layout`] describes a frame's geometry, [`hints`]
//! works out which description applies, and [`audio`] says what a track's channels are.
//!
//! The engine ([`player`]) is FFmpeg with each platform's own decoder under it, where the port
//! had libmpv; [`screen`] draws what it decodes.

pub mod audio;
pub mod hints;
pub mod layout;
pub mod mesh;

#[cfg(feature = "engine")]
pub mod player;
#[cfg(feature = "engine")]
pub mod screen;
#[cfg(feature = "engine")]
mod sound;

pub use audio::{AudioSummary, SPATIAND_LAYOUTS};
pub use hints::{ContainerHints, ContainerStereo};
pub use layout::{Eye, Packing, Projection, StereoLayout, StereoOutput, VideoLayout};
pub use mesh::{DrawMode, Mesh};
