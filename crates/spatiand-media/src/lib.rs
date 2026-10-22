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
pub mod hardware;
pub mod hints;
pub mod layout;
pub mod mesh;
pub mod stretch;
pub mod subs;

#[cfg(feature = "engine")]
pub mod fetch;
#[cfg(feature = "engine")]
pub mod mse;
#[cfg(feature = "engine")]
pub mod player;
#[cfg(feature = "engine")]
pub mod screen;
#[cfg(feature = "engine")]
mod sound;
#[cfg(feature = "engine")]
pub use sound::{SoundGroup, Stream};
#[cfg(feature = "engine")]
pub mod still;

/// **Where the trusted certificates are, for FFmpeg's own https** (a streaming playlist, a film
/// the app's own fetching left to FFmpeg). The OpenSSL Spatiand ships was built to look in its
/// own build folder, and so trusted nobody: Apple's own test stream was refused,
/// "certificate verify failed". The system's list is named instead, unless one is set already.
#[cfg(feature = "engine")]
pub(crate) fn trusted_certificates() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        if std::env::var_os("SSL_CERT_FILE").is_some() || std::env::var_os("SSL_CERT_DIR").is_some() {
            return;
        }
        // macOS keeps its roots for OpenSSL-style readers here; Linux distributions in one of these.
        let files = ["/etc/ssl/cert.pem", "/etc/ssl/certs/ca-certificates.crt", "/etc/pki/tls/certs/ca-bundle.crt"];
        if let Some(file) = files.iter().find(|f| std::path::Path::new(f).exists()) {
            // Before any thread of the media's reads it: the first film opened is the first use.
            unsafe { std::env::set_var("SSL_CERT_FILE", file) };
        }
    });
}

pub use audio::{AudioSummary, SPATIAND_LAYOUTS};
pub use hints::{ContainerHints, ContainerStereo};
pub use layout::{Eye, Packing, Projection, StereoLayout, StereoOutput, VideoLayout};
pub use mesh::{DrawMode, Mesh};
