//! SpatiKey: Spatiand's passwords and passkeys. docs/spatikey.md is the design.
//!
//! The vault lives in the applications' synced folder, encrypted, one file per entry. Only the
//! service reads it, and only with the vault key, which a finger on a Mac (or the recovery code)
//! gives. Everything that wants a password or a passkey -- SpatiKey's window, SpatiMesh, the
//! browser extensions, the room -- asks the service over one local socket, in the protocol of
//! [`wire`]. On a machine with no fingerprint reader the service forwards each request to a Mac
//! on the same network ([`remote`]).

pub mod b64;
pub mod cbor;
pub mod entry;
pub mod hooks;
pub mod lan;
pub mod paths;
pub mod site;
pub mod socket;
pub mod totp;
pub mod webauthn;
pub mod wire;

// The service, and what only it needs: the vault, its keys, and the network to the Mac. A
// program that only asks it -- SpatiMesh, SpatiKey's window, the browsers' host -- leaves it out
// (`default-features = false`) and asks over the socket.
#[cfg(feature = "service")]
pub mod recovery;
#[cfg(feature = "service")]
pub mod remote;
#[cfg(feature = "service")]
pub mod service;
#[cfg(feature = "service")]
pub mod vault;

pub use entry::{Entry, Kind, Passkey, Summary, Totp};
pub use wire::{Code, Envelope, Refusal, Reply, Request, Who};

/// Milliseconds since 1970, the time every file and entry is stamped with.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
