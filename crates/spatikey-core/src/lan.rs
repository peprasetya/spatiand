//! What "the same network" means, and the broadcast that proves it. docs/spatikey.md, "The Deck
//! and the Beam Pro ask the Mac".
//!
//! An address proves nothing here. Tailscale's subnet routes carry the very private ranges the
//! home network uses, so a packet from 192.168.1.20 may have come out of a tunnel, from a router
//! standing in for a device somewhere else. What no router and no tunnel carries is a broadcast.
//! So a device is on this machine's network when its broadcast arrived here, sent to a broadcast
//! address, on an interface that can broadcast, from inside that interface's own subnet. The
//! interfaces are judged by their flags and addresses, never by their names: `utun7` today is
//! `utun4` after a reboot, and a name says nothing about where a packet came from.

use std::ffi::CStr;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddrV4, UdpSocket};

/// Where devices broadcast to find a Mac.
pub const DISCOVERY_PORT: u16 = 47611;
/// Where the Mac takes requests (QUIC).
pub const ASK_PORT: u16 = 47612;

/// The longest beacon, in bytes. A broadcast is never fragmented, and nothing a beacon says needs
/// more than one small packet.
pub const MAX_BEACON: usize = 512;
/// The longest name a beacon carries, in bytes before percent-encoding. [`Beacon::encode`]
/// shortens a longer one, so that even a name of three-byte characters, each sent as `%XX`
/// three times over, leaves the beacon under [`MAX_BEACON`].
pub const MAX_NAME: usize = 128;

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
    listed().iter().filter_map(lan_from).collect()
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
    match canonical(ip) {
        IpAddr::V4(v4) => in_subnet(IpAddr::V4(v4), IpAddr::V4(Ipv4Addr::new(100, 64, 0, 0)), 10),
        IpAddr::V6(v6) => in_subnet(IpAddr::V6(v6), IpAddr::V6(Ipv6Addr::new(0xfd7a, 0x115c, 0xa1e0, 0, 0, 0, 0, 0)), 48),
    }
}

/// Is `addr` inside `net`/`prefix`? Both are read as [`canonical`] first, so an IPv4-mapped
/// address is the IPv4 address it carries. Addresses of different families are never inside each
/// other, and a prefix longer than the address is no subnet at all.
pub fn in_subnet(addr: IpAddr, net: IpAddr, prefix: u8) -> bool {
    // A mapped network keeps its prefix in IPv6 terms: ::ffff:192.168.1.0/120 is 192.168.1.0/24.
    let prefix = match net {
        IpAddr::V6(v6) if v6.to_ipv4_mapped().is_some() => match prefix.checked_sub(96) {
            Some(prefix) => prefix,
            None => return false,
        },
        _ => prefix,
    };
    match (canonical(addr), canonical(net)) {
        (IpAddr::V4(a), IpAddr::V4(n)) => {
            if prefix > 32 {
                return false;
            }
            let mask = u32::MAX.checked_shl(32 - prefix as u32).unwrap_or(0);
            u32::from(a) & mask == u32::from(n) & mask
        }
        (IpAddr::V6(a), IpAddr::V6(n)) => {
            if prefix > 128 {
                return false;
            }
            let mask = u128::MAX.checked_shl(128 - prefix as u32).unwrap_or(0);
            u128::from(a) & mask == u128::from(n) & mask
        }
        _ => false,
    }
}

/// The LAN interface whose subnet holds `peer`, if any.
pub fn lan_of(peer: IpAddr) -> Option<Lan> {
    lan_in(&interfaces(), peer).cloned()
}

/// The LAN in `lans` whose subnet holds `peer` -- the narrowest, when several do -- and never
/// for a tunnel's address, whatever subnet it seems to sit in.
pub fn lan_in(lans: &[Lan], peer: IpAddr) -> Option<&Lan> {
    let peer = canonical(peer);
    if is_tunnel(peer) {
        return None;
    }
    lans.iter().filter(|lan| in_subnet(peer, lan.addr, lan.prefix)).max_by_key(|lan| lan.prefix)
}

/// The LAN a broadcast counts on: one sent to 255.255.255.255 or to that LAN's own broadcast
/// address, that came in by that LAN's interface, from inside its subnet. None for anything
/// else -- a packet sent to this machine's own address (which anyone anywhere can send), one
/// that came in by another interface than the subnet its source claims, one from a tunnel's
/// address.
pub fn heard_on(lans: &[Lan], source: IpAddr, destination: Ipv4Addr, arrived_on: u32) -> Option<&Lan> {
    let source = canonical(source);
    if is_tunnel(source) {
        return None;
    }
    lans.iter().find(|lan| {
        lan.index == arrived_on
            && (destination == Ipv4Addr::BROADCAST || lan.broadcast == Some(destination))
            && in_subnet(source, lan.addr, lan.prefix)
    })
}

/// The system's index for the interface `name` -- the number a packet's arrival is reported by.
pub fn index_of(name: &str) -> Option<u32> {
    let name = std::ffi::CString::new(name).ok()?;
    // SAFETY: a NUL-terminated name, only read.
    let index = unsafe { libc::if_nametoindex(name.as_ptr()) };
    (index != 0).then_some(index)
}

// MARK: the interfaces

/// One address of one interface, as the system lists it.
#[derive(Debug, Clone)]
struct Listed {
    name: String,
    index: u32,
    flags: u32,
    addr: IpAddr,
    mask: Option<IpAddr>,
    broadcast: Option<Ipv4Addr>,
}

/// Whether one listed address is a LAN's, and the LAN it is.
fn lan_from(listed: &Listed) -> Option<Lan> {
    let flag = |f: libc::c_int| listed.flags & f as u32 != 0;
    // Up and running, able to broadcast; not loopback, not a point-to-point link (every tunnel
    // here is one: utun on the Mac, tailscale0 and wg on Linux, tun0 on Android).
    if !flag(libc::IFF_UP) || !flag(libc::IFF_RUNNING) || !flag(libc::IFF_BROADCAST) {
        return None;
    }
    if flag(libc::IFF_LOOPBACK) || flag(libc::IFF_POINTOPOINT) {
        return None;
    }
    if listed.index == 0 {
        return None;
    }
    let addr = listed.addr;
    let unusable = match addr {
        IpAddr::V4(v4) => v4.is_unspecified() || v4.is_loopback() || v4.is_multicast() || v4.is_link_local(),
        IpAddr::V6(v6) => {
            v6.is_unspecified()
                || v6.is_loopback()
                || v6.is_multicast()
                || v6.to_ipv4_mapped().is_some()
                // fe80::/10: on every interface, routed nowhere, proves nothing.
                || (v6.segments()[0] & 0xffc0) == 0xfe80
        }
    };
    if unusable || is_tunnel(addr) {
        return None;
    }
    let prefix = match listed.mask? {
        IpAddr::V4(mask) if addr.is_ipv4() => u32::from(mask).leading_ones() as u8,
        IpAddr::V6(mask) if addr.is_ipv6() => u128::from(mask).leading_ones() as u8,
        _ => return None,
    };
    // No home network is a /7. A mask that short is one misread -- or one that would make the
    // subnet test pass for every address there is -- so the address does not count.
    let shortest = if addr.is_ipv4() { 8 } else { 16 };
    if prefix < shortest {
        return None;
    }
    let broadcast = match addr {
        IpAddr::V4(v4) => listed.broadcast.filter(|b| !b.is_unspecified()).or_else(|| {
            // Some systems leave it out; it is the subnet with every host bit set.
            (prefix <= 30).then(|| Ipv4Addr::from(u32::from(v4) | (u32::MAX >> prefix)))
        }),
        IpAddr::V6(_) => None,
    };
    Some(Lan { name: listed.name.clone(), index: listed.index, addr, prefix, broadcast })
}

/// Every address of every interface, from getifaddrs.
fn listed() -> Vec<Listed> {
    let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: getifaddrs either fails or hands over a list, which freeifaddrs gives back below.
    if unsafe { libc::getifaddrs(&mut head) } != 0 {
        log::warn!("could not list this machine's network interfaces: {}", std::io::Error::last_os_error());
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut at = head;
    while !at.is_null() {
        // SAFETY: a node of the list getifaddrs made, alive until freeifaddrs.
        let node = unsafe { &*at };
        at = node.ifa_next;
        // SAFETY: the node's pointers are null or point at what getifaddrs filled in.
        if let Some(listed) = unsafe { read_node(node) } {
            out.push(listed);
        }
    }
    // SAFETY: the list getifaddrs made, given back once.
    unsafe { libc::freeifaddrs(head) };
    out
}

/// # Safety
/// `node` comes from getifaddrs, and the list it is in has not been freed.
unsafe fn read_node(node: &libc::ifaddrs) -> Option<Listed> {
    if node.ifa_name.is_null() {
        return None;
    }
    let addr = ip_of(node.ifa_addr)?;
    let flags = node.ifa_flags as u32;
    let mask = mask_of(node.ifa_netmask, addr);
    #[cfg(any(target_os = "linux", target_os = "android"))]
    let other = node.ifa_ifu;
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    let other = node.ifa_dstaddr;
    // The same field is the far end of a point-to-point link; it is a broadcast address only
    // when the interface says it broadcasts.
    let broadcast = match (flags & libc::IFF_BROADCAST as u32 != 0, addr) {
        (true, IpAddr::V4(_)) => match ip_of(other) {
            Some(IpAddr::V4(b)) => Some(b),
            _ => None,
        },
        _ => None,
    };
    Some(Listed {
        name: CStr::from_ptr(node.ifa_name).to_string_lossy().into_owned(),
        index: libc::if_nametoindex(node.ifa_name),
        flags,
        addr,
        mask,
        broadcast,
    })
}

/// # Safety
/// `sa` is null or points at a whole sockaddr of the family it names.
unsafe fn ip_of(sa: *const libc::sockaddr) -> Option<IpAddr> {
    if sa.is_null() {
        return None;
    }
    match (*sa).sa_family as libc::c_int {
        libc::AF_INET => {
            let sin = std::ptr::read_unaligned(sa as *const libc::sockaddr_in);
            Some(IpAddr::V4(Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr))))
        }
        libc::AF_INET6 => {
            let sin6 = std::ptr::read_unaligned(sa as *const libc::sockaddr_in6);
            Some(IpAddr::V6(Ipv6Addr::from(sin6.sin6_addr.s6_addr)))
        }
        _ => None,
    }
}

/// A netmask, in the family of the address it belongs to.
///
/// BSD hands masks over trimmed -- `sa_len` short of the whole sockaddr, sometimes with the
/// family left at 0 -- and the bytes left off are zeros. So the mask is read only as far as
/// `sa_len` says, and its family is taken from the address.
///
/// # Safety
/// `sa` is null or points at a netmask getifaddrs filled in.
unsafe fn mask_of(sa: *const libc::sockaddr, addr: IpAddr) -> Option<IpAddr> {
    if sa.is_null() {
        return None;
    }
    let (whole, at, width) = match addr {
        IpAddr::V4(_) => (
            std::mem::size_of::<libc::sockaddr_in>(),
            std::mem::offset_of!(libc::sockaddr_in, sin_addr),
            4,
        ),
        IpAddr::V6(_) => (
            std::mem::size_of::<libc::sockaddr_in6>(),
            std::mem::offset_of!(libc::sockaddr_in6, sin6_addr),
            16,
        ),
    };
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    let length = ((*sa).sa_len as usize).min(whole);
    #[cfg(any(target_os = "linux", target_os = "android"))]
    let length = whole;
    let mut bytes = [0u8; std::mem::size_of::<libc::sockaddr_in6>()];
    std::ptr::copy_nonoverlapping(sa as *const u8, bytes.as_mut_ptr(), length);
    let mut mask = [0u8; 16];
    mask[..width].copy_from_slice(&bytes[at..at + width]);
    Some(match addr {
        IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::new(mask[0], mask[1], mask[2], mask[3])),
        IpAddr::V6(_) => IpAddr::V6(Ipv6Addr::from(mask)),
    })
}

// MARK: hearing a broadcast

/// A datagram as the discovery socket received it, with what the system says of it: the
/// destination address in its header, and the interface it came in by. Either is None when the
/// system did not say, and then the datagram proves nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Arrival {
    /// Its length, at the front of the buffer it was received into.
    pub len: usize,
    pub from: SocketAddrV4,
    pub to: Option<Ipv4Addr>,
    pub on: Option<u32>,
}

/// The socket a Mac hears broadcasts on: UDP `port` (0 for any) on every IPv4 address, shared
/// with whoever else listens there (SO_REUSEADDR, SO_REUSEPORT), allowed to broadcast, and asking
/// the system for each datagram's destination and interface -- IP_RECVDSTADDR and IP_RECVIF on
/// the Mac, IP_PKTINFO on Linux and Android -- without which no broadcast can be told from a
/// packet sent to this machine's own address.
pub fn listen(port: u16) -> std::io::Result<UdpSocket> {
    use std::os::fd::FromRawFd;

    #[cfg(any(target_os = "linux", target_os = "android"))]
    let kind = libc::SOCK_DGRAM | libc::SOCK_CLOEXEC;
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    let kind = libc::SOCK_DGRAM;
    // SAFETY: a new socket, owned by the UdpSocket from here on (which closes it).
    let fd = unsafe { libc::socket(libc::AF_INET, kind, 0) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: fd is a socket this function just made and nothing else owns.
    let socket = unsafe { UdpSocket::from_raw_fd(fd) };
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    {
        // Not left open in the programs Spatiand starts.
        // SAFETY: fcntl on a descriptor this function owns.
        if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    set_option(fd, libc::SOL_SOCKET, libc::SO_REUSEADDR)?;
    set_option(fd, libc::SOL_SOCKET, libc::SO_REUSEPORT)?;
    set_option(fd, libc::SOL_SOCKET, libc::SO_BROADCAST)?;
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    {
        set_option(fd, libc::IPPROTO_IP, libc::IP_RECVDSTADDR)?;
        set_option(fd, libc::IPPROTO_IP, libc::IP_RECVIF)?;
    }
    #[cfg(any(target_os = "linux", target_os = "android"))]
    set_option(fd, libc::IPPROTO_IP, libc::IP_PKTINFO)?;

    // SAFETY: a zeroed sockaddr_in is a valid one, filled in below.
    let mut address: libc::sockaddr_in = unsafe { std::mem::zeroed() };
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    {
        address.sin_len = std::mem::size_of::<libc::sockaddr_in>() as u8;
    }
    address.sin_family = libc::AF_INET as libc::sa_family_t;
    address.sin_port = port.to_be();
    address.sin_addr = libc::in_addr { s_addr: u32::from(Ipv4Addr::UNSPECIFIED).to_be() };
    // SAFETY: binding a socket this function owns to an address that lives across the call.
    let bound = unsafe {
        libc::bind(
            fd,
            &address as *const libc::sockaddr_in as *const libc::sockaddr,
            std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t,
        )
    };
    if bound != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(socket)
}

fn set_option(fd: libc::c_int, level: libc::c_int, name: libc::c_int) -> std::io::Result<()> {
    let on: libc::c_int = 1;
    // SAFETY: setsockopt reads one int that lives across the call.
    let result = unsafe {
        libc::setsockopt(
            fd,
            level,
            name,
            &on as *const libc::c_int as *const libc::c_void,
            std::mem::size_of::<libc::c_int>() as libc::socklen_t,
        )
    };
    if result != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// Receive one datagram on a [`listen`] socket into `buffer`. One longer than the buffer is an
/// error of kind `InvalidData`; so is one from anything but an IPv4 address. A read timeout set on
/// the socket comes back as `WouldBlock` (or `TimedOut`), as from any socket.
pub fn receive(socket: &UdpSocket, buffer: &mut [u8]) -> std::io::Result<Arrival> {
    use std::os::fd::AsRawFd;

    // SAFETY: zeroed sockaddr_in and msghdr are valid, and are filled in below.
    let mut from: libc::sockaddr_in = unsafe { std::mem::zeroed() };
    let mut iov = libc::iovec { iov_base: buffer.as_mut_ptr() as *mut libc::c_void, iov_len: buffer.len() };
    // Room for the control messages, aligned as a cmsghdr wants.
    let mut control = [0u64; 32];
    // SAFETY: as above.
    let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
    message.msg_name = &mut from as *mut libc::sockaddr_in as *mut libc::c_void;
    message.msg_namelen = std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t;
    message.msg_iov = &mut iov;
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr() as *mut libc::c_void;
    message.msg_controllen = std::mem::size_of_val(&control) as _;
    // SAFETY: every pointer in `message` points at a buffer above that outlives the call.
    let length = unsafe { libc::recvmsg(socket.as_raw_fd(), &mut message, 0) };
    if length < 0 {
        return Err(std::io::Error::last_os_error());
    }
    if message.msg_flags & libc::MSG_TRUNC != 0 {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "a datagram longer than any beacon"));
    }
    if from.sin_family as libc::c_int != libc::AF_INET {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "a datagram from no IPv4 address"));
    }
    let mut arrival = Arrival {
        len: length as usize,
        from: SocketAddrV4::new(Ipv4Addr::from(u32::from_be(from.sin_addr.s_addr)), u16::from_be(from.sin_port)),
        to: None,
        on: None,
    };
    // Cut short, the control messages cannot be trusted to say all they would have.
    if message.msg_flags & libc::MSG_CTRUNC != 0 {
        return Ok(arrival);
    }
    // SAFETY: walking the control messages the system wrote into `control`, by its own macros,
    // and reading each one's data only as far as its length says.
    unsafe {
        let mut cmsg = libc::CMSG_FIRSTHDR(&message);
        while !cmsg.is_null() {
            let header = &*cmsg;
            let data = libc::CMSG_DATA(cmsg);
            let room = (header.cmsg_len as usize).saturating_sub(data as usize - cmsg as usize);
            if header.cmsg_level == libc::IPPROTO_IP {
                read_control(header.cmsg_type, data, room, &mut arrival);
            }
            cmsg = libc::CMSG_NXTHDR(&message, cmsg);
        }
    }
    Ok(arrival)
}

/// # Safety
/// `data` points at `room` readable bytes of a control message of type `kind`.
#[cfg(any(target_os = "macos", target_os = "ios"))]
unsafe fn read_control(kind: libc::c_int, data: *const u8, room: usize, arrival: &mut Arrival) {
    match kind {
        libc::IP_RECVDSTADDR if room >= std::mem::size_of::<libc::in_addr>() => {
            let to = std::ptr::read_unaligned(data as *const libc::in_addr);
            arrival.to = Some(Ipv4Addr::from(u32::from_be(to.s_addr)));
        }
        // A sockaddr_dl, of which only the fixed head is needed: its sdl_index.
        libc::IP_RECVIF if room >= std::mem::offset_of!(libc::sockaddr_dl, sdl_type) => {
            let index = std::ptr::read_unaligned(data.add(std::mem::offset_of!(libc::sockaddr_dl, sdl_index)) as *const u16);
            arrival.on = (index != 0).then_some(index as u32);
        }
        _ => {}
    }
}

/// # Safety
/// `data` points at `room` readable bytes of a control message of type `kind`.
#[cfg(any(target_os = "linux", target_os = "android"))]
unsafe fn read_control(kind: libc::c_int, data: *const u8, room: usize, arrival: &mut Arrival) {
    if kind == libc::IP_PKTINFO && room >= std::mem::size_of::<libc::in_pktinfo>() {
        let info = std::ptr::read_unaligned(data as *const libc::in_pktinfo);
        // ipi_addr is the destination in the packet's header; ipi_spec_dst is only the address
        // a reply would come from.
        arrival.to = Some(Ipv4Addr::from(u32::from_be(info.ipi_addr.s_addr)));
        arrival.on = (info.ipi_ifindex > 0).then_some(info.ipi_ifindex as u32);
    }
}

// MARK: beacons

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

const MAGIC: &str = "SPATIKEY1";

impl Beacon {
    /// The line to send. The nonce and fingerprint go as they are (lowercase hex, which is all
    /// [`Beacon::decode`] takes); the name loses its control characters, is cut to [`MAX_NAME`]
    /// bytes and is percent-encoded.
    pub fn encode(&self) -> String {
        match self {
            Beacon::Ask { nonce, fp, name } => format!("{MAGIC} ASK {nonce} {fp} {}", encode_name(name)),
            Beacon::Here { nonce, fp, port, name } => format!("{MAGIC} HERE {nonce} {fp} {port} {}", encode_name(name)),
            Beacon::Pair { nonce, fp, name } => format!("{MAGIC} PAIR {nonce} {fp} {}", encode_name(name)),
        }
    }

    /// None for anything that is not a well-formed beacon (wrong magic, a nonce that is not 16
    /// hex, a fingerprint that is not 64 hex, a port that is not a number).
    ///
    /// Strictly: at most [`MAX_BEACON`] bytes of printable ASCII, words parted by single spaces,
    /// exactly as many words as the kind has, lowercase hex, a port of 1 to 65535 written without
    /// leading zeros, and a name of unreserved characters and `%XX` escapes that decodes to at
    /// most [`MAX_NAME`] bytes of UTF-8 without control characters.
    pub fn decode(text: &str) -> Option<Beacon> {
        if text.len() > MAX_BEACON || !text.bytes().all(|b| b == b' ' || b.is_ascii_graphic()) {
            return None;
        }
        let words: Vec<&str> = text.split(' ').collect();
        if words[0] != MAGIC {
            return None;
        }
        match (words.get(1).copied(), words.len()) {
            (Some("ASK"), 5) => Some(Beacon::Ask {
                nonce: lower_hex(words[2], 16)?,
                fp: lower_hex(words[3], 64)?,
                name: decode_name(words[4])?,
            }),
            (Some("PAIR"), 5) => Some(Beacon::Pair {
                nonce: lower_hex(words[2], 16)?,
                fp: lower_hex(words[3], 64)?,
                name: decode_name(words[4])?,
            }),
            (Some("HERE"), 6) => Some(Beacon::Here {
                nonce: lower_hex(words[2], 16)?,
                fp: lower_hex(words[3], 64)?,
                port: port(words[4])?,
                name: decode_name(words[5])?,
            }),
            _ => None,
        }
    }
}

fn lower_hex(word: &str, length: usize) -> Option<String> {
    (word.len() == length && word.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))).then(|| word.to_string())
}

fn port(word: &str) -> Option<u16> {
    if word.is_empty() || word.len() > 5 || word.starts_with('0') || !word.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    word.parse().ok()
}

fn unreserved(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~')
}

fn encode_name(name: &str) -> String {
    let mut kept = String::new();
    for c in name.chars().filter(|c| !c.is_control()) {
        if kept.len() + c.len_utf8() > MAX_NAME {
            break;
        }
        kept.push(c);
    }
    let mut out = String::with_capacity(kept.len() * 3);
    for byte in kept.bytes() {
        if unreserved(byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn decode_name(word: &str) -> Option<String> {
    fn hex(byte: u8) -> Option<u8> {
        (byte as char).to_digit(16).map(|d| d as u8)
    }
    let bytes = word.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let high = hex(*bytes.get(i + 1)?)?;
            let low = hex(*bytes.get(i + 2)?)?;
            out.push(high << 4 | low);
            i += 3;
        } else if unreserved(bytes[i]) {
            out.push(bytes[i]);
            i += 1;
        } else {
            return None;
        }
    }
    if out.len() > MAX_NAME {
        return None;
    }
    let name = String::from_utf8(out).ok()?;
    (!name.chars().any(|c| c.is_control())).then_some(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listed(name: &str, index: u32, flags: libc::c_int, addr: &str, mask: &str, broadcast: Option<&str>) -> Listed {
        Listed {
            name: name.into(),
            index,
            flags: flags as u32,
            addr: addr.parse().unwrap(),
            mask: Some(mask.parse().unwrap()),
            broadcast: broadcast.map(|b| b.parse().unwrap()),
        }
    }

    /// This Mac as it was measured on 2026-10-29: Wi-Fi on en0, Tailscale on utun7 (which also
    /// routes the home and office subnets), the T2 bridge on en5 with only a link-local address,
    /// loopback. Only en0's IPv4 address is a LAN.
    #[test]
    fn of_this_macs_interfaces_only_the_wi_fi_counts() {
        use libc::{IFF_BROADCAST as B, IFF_LOOPBACK as L, IFF_MULTICAST as M, IFF_POINTOPOINT as P, IFF_RUNNING as R, IFF_UP as U};
        let measured = [
            listed("lo0", 1, U | L | R | M, "127.0.0.1", "255.0.0.0", None),
            listed("en0", 11, U | B | R | M, "192.168.1.71", "255.255.255.0", Some("192.168.1.255")),
            listed("en0", 11, U | B | R | M, "fe80::1c2a:3bff:fe4d:5e6f", "ffff:ffff:ffff:ffff::", None),
            listed("en5", 7, U | B | R | M, "fe80::aede:48ff:fe00:1122", "ffff:ffff:ffff:ffff::", None),
            listed("utun7", 25, U | P | R | M, "100.121.56.71", "255.255.255.255", None),
            listed("utun7", 25, U | P | R | M, "fd7a:115c:a1e0::c001:385d", "ffff:ffff:ffff::", None),
            // A tunnel that claimed to broadcast would still be refused by its address.
            listed("odd0", 30, U | B | R | M, "100.100.1.2", "255.255.255.0", Some("100.100.1.255")),
            // A broadcasting interface that is down, or holds a self-assigned address, is not one.
            listed("en7", 12, B | M, "192.168.1.80", "255.255.255.0", Some("192.168.1.255")),
            listed("bridge0", 13, U | B | R | M, "169.254.10.20", "255.255.0.0", Some("169.254.255.255")),
        ];
        let lans: Vec<Lan> = measured.iter().filter_map(lan_from).collect();
        assert_eq!(
            lans,
            vec![Lan {
                name: "en0".into(),
                index: 11,
                addr: "192.168.1.71".parse().unwrap(),
                prefix: 24,
                broadcast: Some(Ipv4Addr::new(192, 168, 1, 255)),
            }]
        );
    }

    #[test]
    fn a_global_ipv6_address_counts_and_a_missing_broadcast_is_worked_out() {
        use libc::{IFF_BROADCAST as B, IFF_RUNNING as R, IFF_UP as U};
        let v6 = lan_from(&listed("en0", 11, U | B | R, "2001:db8:1:2::71", "ffff:ffff:ffff:ffff::", None)).unwrap();
        assert_eq!((v6.prefix, v6.broadcast), (64, None));
        let v4 = lan_from(&listed("eth0", 2, U | B | R, "10.0.5.9", "255.255.252.0", None)).unwrap();
        assert_eq!(v4.broadcast, Some(Ipv4Addr::new(10, 0, 7, 255)));
        assert!(lan_from(&listed("eth0", 2, U | B | R, "10.0.5.9", "254.0.0.0", None)).is_none(), "a /7 is no home network");
    }

    #[test]
    fn names_survive_the_trip_and_are_cut_to_fit() {
        let name = "Paulus’s Steam Deck 100%";
        assert_eq!(decode_name(&encode_name(name)).as_deref(), Some(name));
        assert_eq!(encode_name("a b"), "a%20b");
        let long = "界".repeat(100);
        let encoded = encode_name(&long);
        let back = decode_name(&encoded).unwrap();
        assert!(back.len() <= MAX_NAME && long.starts_with(&back), "cut on a character's edge");
        assert_eq!(encode_name("tab\there"), "tabhere", "control characters are dropped");
    }
}
