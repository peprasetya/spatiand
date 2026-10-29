//! What "the same network" means, and the broadcast that proves it. docs/spatikey.md, "The Deck
//! and the Beam Pro ask the Mac".
//!
//! STUB: the signatures are the contract; the bodies are to be written.

use std::net::{IpAddr, Ipv4Addr};

/// Where devices broadcast to find a Mac.
pub const DISCOVERY_PORT: u16 = 47611;
/// Where the Mac takes requests (QUIC).
pub const ASK_PORT: u16 = 47612;

/// A network interface this machine is on that counts as a LAN: up, able to broadcast, not
/// loopback or point-to-point, with an address that is not a tunnel's. Judged by flags and
/// addresses, never by name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lan {
    pub name: String,
    pub index: u32,
    pub addr: IpAddr,
    pub prefix: u8,
    /// The directed broadcast address (IPv4 only).
    pub broadcast: Option<Ipv4Addr>,
}

/// Every LAN address this machine has, one entry per address.
pub fn interfaces() -> Vec<Lan> {
    todo!()
}

/// An IPv4-mapped IPv6 address as the IPv4 address it is; anything else unchanged.
pub fn canonical(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
        v4 => v4,
    }
}

/// Tailscale's ranges: 100.64.0.0/10 and fd7a:115c:a1e0::/48.
pub fn is_tunnel(ip: IpAddr) -> bool {
    let _ = ip;
    todo!()
}

/// Is `addr` inside `net`/`prefix`?
pub fn in_subnet(addr: IpAddr, net: IpAddr, prefix: u8) -> bool {
    let _ = (addr, net, prefix);
    todo!()
}

/// The LAN interface whose subnet holds `peer`, if any.
pub fn lan_of(peer: IpAddr) -> Option<Lan> {
    let _ = peer;
    todo!()
}

/// One discovery packet: a line of ASCII words. Names are percent-encoded (no spaces).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Beacon {
    /// "SPATIKEY1 ASK <nonce> <fp> <name>": a device looking for Macs.
    Ask { nonce: String, fp: String, name: String },
    /// "SPATIKEY1 HERE <nonce> <fp> <port> <name>": a Mac answering.
    Here { nonce: String, fp: String, port: u16, name: String },
    /// "SPATIKEY1 PAIR <nonce> <fp> <name>": a device about to ask to pair.
    Pair { nonce: String, fp: String, name: String },
}

impl Beacon {
    pub fn encode(&self) -> String {
        todo!()
    }

    /// None for anything that is not a well-formed beacon (wrong magic, a nonce that is not 16
    /// hex, a fingerprint that is not 64 hex, a port that is not a number).
    pub fn decode(text: &str) -> Option<Beacon> {
        let _ = text;
        todo!()
    }
}
