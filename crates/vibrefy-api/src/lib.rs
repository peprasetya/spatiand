//! A client for one Vibrefy deployment.
//!
//! The wire format is the server's own, described in `CLIENT-API.md` in the Vibrefy
//! repository: `Accept: application/json` on every request, a `JSESSIONID` cookie for
//! authentication, and playback URLs that instead carry the session id inline so a media
//! engine can fetch them with no cookie plumbing at all.

pub mod client;
pub mod cookies;
pub mod episodes;
pub mod json;
pub mod lock;
pub mod pairing;
pub mod paths;
pub mod progress;
pub mod store;
pub mod models;

pub use client::{ApiError, SessionState, VibrefyClient};
pub use pairing::{PairingCode, PairingOffer, PairingState};
pub use store::{Account, Config, Settings};
pub use models::{Crumb, FolderListing, ItemGroup, ItemType, MediaItem, SubtitleTrack};
