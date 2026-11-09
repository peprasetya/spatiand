//! Spatiand's own adb client: how the Beam Pro's Spatiand starts its shell helper by itself.
//!
//! What lets the room show Android's own apps -- a display another app runs on, input put into
//! it, its sound taken -- Android gives the shell user alone, and the shell user is reached
//! through adb, which is what a computer uses. But a phone's Wireless debugging has adbd
//! listening on the phone itself, and an app there can be that computer: pair once, with the
//! code Settings shows ([`pair`]), then connect with the key paired ([`Connection`]) and run a
//! command as the shell user. It is what Shizuku does; the owner asked for it in Spatiand
//! itself, rather than as another app to install.
//!
//! - [`spake2`]: BoringSSL's SPAKE2, which adb's pairing runs.
//! - [`key`]: an RSA key as adb knows it.
//! - [`pair`]: adb's pairing protocol.
//! - [`connection`]: adb's transport over TLS, and its `exec:` service.
//!
//! Nothing here is Android's: it is tested on a computer, against BoringSSL and adb's own keys.

pub mod connection;
pub mod key;
pub mod pair;
pub mod spake2;
mod tls;

pub use connection::Connection;
pub use key::AdbKey;
pub use pair::pair;
