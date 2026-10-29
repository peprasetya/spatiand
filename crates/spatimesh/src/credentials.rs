//! SpatiKey's scripts for pages (docs/spatikey.md, "SpatiMesh"): the same two files the browser
//! extensions use, and SpatiMesh's own relay.
//!
//! `webauthn.js` goes into the page's own world: it defines `navigator.credentials` and
//! `PublicKeyCredential` (our WPE has no WebAuthn of its own) and speaks only through events.
//! `forms.js` and `cred-world.js` go into the isolated world `spatimesh-cred`, the only one the
//! message handler `spatimeshCred` is given: they stamp each message with the frame's origin
//! as that world sees it, which the page cannot change, and find, fill and watch login forms.

/// For the page's world.
pub fn page_js() -> String {
    include_str!("../../../extensions/spatikey/src/webauthn.js").to_string()
}

/// For the isolated world `spatimesh-cred`.
pub fn world_js() -> String {
    format!("{}\n{}", include_str!("../../../extensions/spatikey/src/forms.js"), include_str!("credentials/cred-world.js"))
}
