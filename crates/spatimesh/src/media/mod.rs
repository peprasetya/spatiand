//! Pages' media, played by Spatiand's own player rather than the engine's (docs/spatimesh.md,
//! "Media"): what the page is given ([`PAGE_JS`]) and the colour a film's box is painted.
//!
//! The window's half -- the players, and drawing what they show under the page -- is the
//! program's (`browser/media.rs`); this is what both halves agree on.

/// Put into every frame's every document, in the page's own world, before the page's scripts.
pub const PAGE_JS: &str = include_str!("page.js");

/// [`PAGE_JS`] as this machine is to hand it over: told which codecs it decodes only in
/// software, so that pages are not offered them (`spatiand_media::hardware`).
pub fn page_js() -> String {
    // `SPATIMESH_SOFTWARE_CODECS=vp09,av01` says it instead, for a machine that answers wrong.
    let named = std::env::var("SPATIMESH_SOFTWARE_CODECS").ok();
    #[cfg(feature = "media")]
    let soft: Vec<&str> = match named.as_deref() {
        Some(list) => list.split(',').map(str::trim).filter(|c| !c.is_empty()).collect(),
        None => spatiand_media::hardware::software_only(),
    };
    #[cfg(not(feature = "media"))]
    let soft: Vec<&str> = named.as_deref().map(|l| l.split(',').map(str::trim).collect()).unwrap_or_default();
    let list = format!("[{}]", soft.iter().map(|c| format!("'{c}'")).collect::<Vec<_>>().join(","));
    PAGE_JS.replacen("/*SOFT*/[]", &list, 1)
}

/// What a film's box is painted (`page.js`): where the page shows exactly this, the film is
/// seen; where the page draws over it, half seen through, the film shows through by as much.
pub const KEY: [u8; 3] = [1, 254, 3];

/// Whether SpatiMesh plays pages' media itself: `SPATIMESH_MEDIA=engine` leaves it to the
/// engine, as before.
pub fn ours() -> bool {
    cfg!(feature = "media") && std::env::var("SPATIMESH_MEDIA").as_deref() != Ok("engine")
}
