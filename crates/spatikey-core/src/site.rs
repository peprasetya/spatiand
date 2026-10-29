//! Which site a page is, by the Public Suffix List (data/public_suffix_list.dat, the copy in
//! libpsl 0.23.3, MPL-2.0).
//!
//! STUB: the signatures are the contract; the bodies are to be written.

/// The host of an origin ("https://Login.Example.com:8443" -> "login.example.com"), lowercased,
/// IPv6 without brackets. None if it is not an origin with a host.
pub fn host_of(origin: &str) -> Option<String> {
    let _ = origin;
    todo!()
}

/// https, or http to localhost, *.localhost, 127.0.0.0/8 or ::1: what WebAuthn and filling accept.
pub fn is_secure_origin(origin: &str) -> bool {
    let _ = origin;
    todo!()
}

/// Is `domain` itself a public suffix ("com", "co.uk", "github.io", and any single unknown label)?
pub fn is_public_suffix(domain: &str) -> bool {
    let _ = domain;
    todo!()
}

/// The registrable domain (eTLD+1) of a host: "a.b.example.co.uk" -> "example.co.uk". None for
/// an IP address, "localhost", or a host that is itself a public suffix.
pub fn registrable_domain(host: &str) -> Option<String> {
    let _ = host;
    todo!()
}

/// May a page at `origin` use the relying party id `rp_id`? The origin must be secure; the
/// rp id must be the origin's host or a parent domain of it, and not a public suffix (an IP
/// address only as the host itself, "localhost" only for a localhost origin).
pub fn rp_id_ok(rp_id: &str, origin: &str) -> bool {
    let _ = (rp_id, origin);
    todo!()
}

/// Does an entry's site (a registrable domain, or a whole origin) cover `origin`? A domain covers
/// its own host and every host under it; an origin only itself.
pub fn site_matches(site: &str, origin: &str) -> bool {
    let _ = (site, origin);
    todo!()
}

/// What to keep as the site of a login first used at `origin`: its registrable domain, or the
/// whole origin for an IP address, localhost or a host with a port other than the scheme's.
pub fn site_for(origin: &str) -> String {
    let _ = origin;
    todo!()
}
