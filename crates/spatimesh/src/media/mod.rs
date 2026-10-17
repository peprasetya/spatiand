//! Pages' media, played by Spatiand's own player rather than the engine's (docs/spatimesh.md,
//! "Media"): what the page is given ([`PAGE_JS`]) and the colour a film's box is painted.
//!
//! The window's half -- the players, and drawing what they show under the page -- is the
//! program's (`browser/media.rs`); this is what both halves agree on.

/// Put into every frame's every document, in the page's own world, before the page's scripts.
pub const PAGE_JS: &str = include_str!("page.js");

/// What a film's box is painted (`page.js`): where the page shows exactly this, the film is
/// seen; where the page draws over it, half seen through, the film shows through by as much.
pub const KEY: [u8; 3] = [1, 254, 3];

/// Whether SpatiMesh plays pages' media itself: `SPATIMESH_MEDIA=engine` leaves it to the
/// engine, as before.
pub fn ours() -> bool {
    std::env::var("SPATIMESH_MEDIA").as_deref() != Ok("engine")
}
