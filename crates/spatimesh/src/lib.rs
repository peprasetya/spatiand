//! SpatiMesh, Spatiand's web browser: everything of it but the program's own loop.
//!
//! See docs/spatimesh.md. [`engine`] is the one interface the platforms' web engines are put
//! behind; the rest needs no engine at all -- the tabs and windows ([`tabs`]), the address bar
//! ([`address`]), [`profiles`], [`bookmarks`], [`settings`], and where all of it is kept
//! ([`store`]).

pub mod address;
pub mod bookmarks;
pub mod engine;
pub mod profiles;
pub mod settings;
pub mod store;
pub mod tabs;
