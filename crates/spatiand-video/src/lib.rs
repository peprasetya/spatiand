//! Decoding, on the headset's side of the link.
//!
//! One decoder per remote window, turning the packets that arrive into pictures on the GPU. The
//! pictures never touch the CPU: what comes out is a dmabuf, which is what a Wayland surface
//! takes, so a remote window is shown exactly as a local application's window is.
//!
//! ## Built against headers that are not installed
//!
//! The Deck has ffmpeg 7.1's libraries and no headers at all, and nothing is installed on it to
//! fix that. So the headers are vendored, the libraries are the system's own, and the version
//! is pinned to what SteamOS ships. The same trick was needed for libva, and `docs/remote.md`
//! has the incantation.
//!
//! The first attempt used `cros-codecs` over libva — pure Rust, no headers needed at all — and
//! it cannot run here: it opens a decoder by creating a 16x16 probe context, and this GPU
//! refuses any video context smaller than 64x64. That was measured for HEVC and H.264 both
//! before giving up on it.
//!
//! ## What a decoder is fed
//!
//! Whole frames, reassembled by [`spatiand_stream::video`]. A frame that lost a piece is never
//! passed in: a decoder shown a frame with a hole in it does not lose one picture, it produces
//! wreckage until the next keyframe, and that wreckage is the smear people recognise as bad
//! game streaming.

#[cfg(not(any(target_os = "android", target_os = "macos")))]
pub mod convert;
#[cfg(not(any(target_os = "android", target_os = "macos")))]
pub mod decode;
#[cfg(not(any(target_os = "android", target_os = "macos")))]
pub mod export;
#[cfg(not(any(target_os = "android", target_os = "macos")))]
pub mod record;
#[cfg(not(any(target_os = "android", target_os = "macos")))]
pub mod split;
#[cfg(not(any(target_os = "android", target_os = "macos")))]
pub mod voice;

#[cfg(not(any(target_os = "android", target_os = "macos")))]
pub use convert::{Converted, Converter};
#[cfg(not(any(target_os = "android", target_os = "macos")))]
pub use decode::{Decoder, Picture, VideoError};
#[cfg(not(any(target_os = "android", target_os = "macos")))]
pub use split::Units;

/// The same interface on Android, on `MediaCodec`; see the file.
#[cfg(target_os = "android")]
pub mod android;
#[cfg(target_os = "android")]
pub use android::{voice, Converted, Converter, Decoder, Picture, VideoError};

/// The same interface on the Mac, on VideoToolbox; see the file.
#[cfg(target_os = "macos")]
pub mod mac;
#[cfg(target_os = "macos")]
pub use mac::{voice, Converted, Converter, Decoder, Picture, VideoError};

/// Where pictures stay on the GPU and reach the compositor by a placeholder's token rather than
/// as a dmabuf: Android's and the Mac's. The same names in both.
#[cfg(target_os = "android")]
pub use android as placeholder;
#[cfg(target_os = "macos")]
pub use mac as placeholder;
