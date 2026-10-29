//! The Deck and the Beam Pro asking the Mac, on this machine: the beacons, the rule for "the
//! same network", and requests and pairing over QUIC on loopback -- through a stand-in rule,
//! since loopback is the first thing the real one refuses. The broadcast on the real network is
//! the one test left out by default, to be run by hand:
//!
//! ```text
//! cargo test -p spatikey-core --test remote -- --ignored --nocapture a_real_broadcast
//! ```

use std::net::{IpAddr, Ipv4Addr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::json;
use spatiand_stream::transport::{self, Fingerprint};
use spatiand_stream::Identity;
use spatikey_core::lan::{self, Beacon, Lan};
use spatikey_core::remote::{self, Answers, Asker, Heard, Network, Ports};
use spatikey_core::service::ConnId;
use spatikey_core::wire::{self, Approver, Code, Device, Envelope, PairAnswer, Reply, Request, Who};

// MARK: a stand-in for the service

/// Answers every request with its op, says yes or no to pairing as it is told, and remembers
/// what it saw.
struct Fake {
    name: String,
    allowed: Mutex<Vec<Device>>,
    yes: AtomicBool,
    asked: Mutex<Vec<(String, String, String)>>,
    seen: Mutex<Vec<(ConnId, Who, String)>>,
    closed: Mutex<Vec<ConnId>>,
}

impl Fake {
    fn new(name: &str) -> Arc<Fake> {
        Arc::new(Fake {
            name: name.into(),
            allowed: Mutex::new(Vec::new()),
            yes: AtomicBool::new(false),
            asked: Mutex::new(Vec::new()),
            seen: Mutex::new(Vec::new()),
            closed: Mutex::new(Vec::new()),
        })
    }

    fn allow_now(&self, fp: Fingerprint, name: &str) {
        self.allowed.lock().unwrap().push(Device { fp: fp.to_string(), name: name.into(), added: 1 });
    }

    fn ops(&self) -> Vec<String> {
        self.seen.lock().unwrap().iter().map(|(_, _, op)| op.clone()).collect()
    }
}

impl Answers for Fake {
    fn name(&self) -> String {
        self.name.clone()
    }

    fn allowed(&self) -> Vec<Device> {
        self.allowed.lock().unwrap().clone()
    }

    fn allow(&self, device: Device) -> Result<(), String> {
        let mut allowed = self.allowed.lock().unwrap();
        allowed.retain(|d| d.fp != device.fp);
        allowed.push(device);
        Ok(())
    }

    fn allow_device(&self, name: &str, fp: &str, code: &str) -> bool {
        self.asked.lock().unwrap().push((name.into(), fp.into(), code.into()));
        self.yes.load(Ordering::SeqCst)
    }

    fn handle(&self, conn: ConnId, env: Envelope) -> Reply {
        let op = serde_json::to_value(&env.request)
            .ok()
            .and_then(|v| v.get("op").and_then(|op| op.as_str()).map(String::from))
            .unwrap_or_default();
        self.seen.lock().unwrap().push((conn, env.who.clone(), op.clone()));
        Reply::ok(env.id, json!({ "op": op }))
    }

    fn closed(&self, conn: ConnId) {
        self.closed.lock().unwrap().push(conn);
    }
}

// MARK: helpers

fn ip(text: &str) -> IpAddr {
    text.parse().unwrap()
}

fn loopback_lan(broadcast: Option<Ipv4Addr>) -> Lan {
    let name = if cfg!(any(target_os = "macos", target_os = "ios")) { "lo0" } else { "lo" };
    Lan {
        name: name.into(),
        index: lan::index_of(name).expect("a loopback interface"),
        addr: IpAddr::V4(Ipv4Addr::LOCALHOST),
        prefix: 8,
        broadcast,
    }
}

/// Loopback standing in for a LAN, its "broadcast address" 127.0.0.1: a beacon sent there
/// arrives as a real broadcast would, through the same socket and the same checks.
fn loopback() -> Network {
    Network::fixed(vec![loopback_lan(Some(Ipv4Addr::LOCALHOST))])
}

fn any_ports() -> Ports {
    Ports { discovery: 0, ask: 0 }
}

fn folder(what: &str) -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "spatikey-remote-{}-{what}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_approvers(dir: &Path, macs: &[Device]) {
    std::fs::write(dir.join("approvers.json"), serde_json::to_vec(macs).unwrap()).unwrap();
}

fn status(id: u64) -> Envelope {
    Envelope { id, who: Who { app: "SpatiMesh".into(), device: String::new() }, request: Request::Status }
}

fn refusal(reply: &Reply) -> Option<Code> {
    reply.err.as_ref().map(|r| r.code)
}

fn nonce() -> String {
    "0123456789abcdef".into()
}

fn wait_for(limit: Duration, done: impl Fn() -> bool) -> bool {
    let until = Instant::now() + limit;
    while Instant::now() < until {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    done()
}

/// Send one beacon to 127.0.0.1:`port` and return the answer, if one comes.
fn beacon(port: u16, beacon: &Beacon) -> Option<Beacon> {
    let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    socket.set_read_timeout(Some(Duration::from_millis(700))).unwrap();
    socket.send_to(beacon.encode().as_bytes(), (Ipv4Addr::LOCALHOST, port)).unwrap();
    let mut buffer = [0u8; 1024];
    let (length, _) = socket.recv_from(&mut buffer).ok()?;
    Beacon::decode(std::str::from_utf8(&buffer[..length]).ok()?)
}

/// One request straight to the Mac's request port, as a device would send it but with no
/// broadcast of its own first unless the test sends one.
fn raw_request(me: &Identity, mac: Fingerprint, port: u16, env: &Envelope) -> Result<Reply, String> {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    runtime.block_on(async {
        let endpoint = transport::client_with(me, vec![mac], &[remote::ALPN], "127.0.0.1:0".parse().unwrap())?;
        let connection = endpoint
            .connect((Ipv4Addr::LOCALHOST, port).into(), "spatiand")
            .map_err(|e| e.to_string())?
            .await
            .map_err(|e| e.to_string())?;
        let (mut send, mut recv) = connection.open_bi().await.map_err(|e| e.to_string())?;
        let mut framed = Vec::new();
        wire::write_frame(&mut framed, &serde_json::to_vec(env).unwrap()).unwrap();
        send.write_all(&framed).await.map_err(|e| e.to_string())?;
        let _ = send.finish();
        let mut length = [0u8; 4];
        recv.read_exact(&mut length).await.map_err(|e| e.to_string())?;
        let mut body = vec![0u8; u32::from_be_bytes(length) as usize];
        recv.read_exact(&mut body).await.map_err(|e| e.to_string())?;
        connection.close(0u32.into(), b"done");
        serde_json::from_slice(&body).map_err(|e| e.to_string())
    })
}

// MARK: beacons

#[test]
fn beacons_survive_the_trip() {
    let fp = "ab".repeat(32);
    let beacons = [
        Beacon::Ask { nonce: nonce(), fp: fp.clone(), name: "Steam Deck".into() },
        Beacon::Here { nonce: nonce(), fp: fp.clone(), port: 47612, name: "darkPhoenix".into() },
        Beacon::Pair { nonce: nonce(), fp: fp.clone(), name: "Beam Pro, 100% “ünïcode”".into() },
        Beacon::Ask { nonce: nonce(), fp: fp.clone(), name: String::new() },
        Beacon::Here { nonce: nonce(), fp: fp.clone(), port: 1, name: "a-b.c_d~e".into() },
    ];
    for b in beacons {
        let text = b.encode();
        assert!(text.len() <= lan::MAX_BEACON && text.is_ascii(), "{text}");
        assert_eq!(Beacon::decode(&text), Some(b), "{text}");
    }
    assert_eq!(
        Beacon::Ask { nonce: nonce(), fp: fp.clone(), name: "Steam Deck".into() }.encode(),
        format!("SPATIKEY1 ASK {} {fp} Steam%20Deck", nonce())
    );
    // A name too long for one packet is cut on a character's edge, and the beacon still reads.
    let text = Beacon::Here { nonce: nonce(), fp, port: 65535, name: "名".repeat(200) }.encode();
    assert!(text.len() <= lan::MAX_BEACON, "{} bytes", text.len());
    match Beacon::decode(&text) {
        Some(Beacon::Here { name, port: 65535, .. }) => {
            assert!(!name.is_empty() && name.len() <= lan::MAX_NAME && name.chars().all(|c| c == '名'))
        }
        other => panic!("read as {other:?}"),
    }
}

#[test]
fn junk_is_not_a_beacon() {
    let n = nonce();
    let f = "ab".repeat(32);
    assert!(Beacon::decode(&format!("SPATIKEY1 ASK {n} {f} Deck")).is_some());
    let junk = [
        String::new(),
        "SPATIKEY1".into(),
        format!("SPATIKEY2 ASK {n} {f} Deck"),
        format!("spatikey1 ASK {n} {f} Deck"),
        format!("SPATIKEY1 ask {n} {f} Deck"),
        format!("SPATIKEY1 HELLO {n} {f} Deck"),
        format!("SPATIKEY1 ASK {n} {f}"),
        format!("SPATIKEY1 ASK {n} {f} Steam Deck"),
        format!("SPATIKEY1 ASK {n} {f} Deck "),
        format!("SPATIKEY1  ASK {n} {f} Deck"),
        format!("SPATIKEY1 ASK {n} {f} Deck\n"),
        format!("SPATIKEY1\tASK {n} {f} Deck"),
        format!("SPATIKEY1 ASK 0123456789abcde {f} Deck"),
        format!("SPATIKEY1 ASK 0123456789abcdef0 {f} Deck"),
        format!("SPATIKEY1 ASK 0123456789ABCDEF {f} Deck"),
        format!("SPATIKEY1 ASK 0123456789abcdeg {f} Deck"),
        format!("SPATIKEY1 ASK {n} {} Deck", "ab".repeat(31)),
        format!("SPATIKEY1 ASK {n} {}zz Deck", "ab".repeat(31)),
        format!("SPATIKEY1 ASK {n} {} Deck", "AB".repeat(32)),
        format!("SPATIKEY1 HERE {n} {f} 0 Mac"),
        format!("SPATIKEY1 HERE {n} {f} 65536 Mac"),
        format!("SPATIKEY1 HERE {n} {f} 047612 Mac"),
        format!("SPATIKEY1 HERE {n} {f} +4761 Mac"),
        format!("SPATIKEY1 HERE {n} {f} port Mac"),
        format!("SPATIKEY1 HERE {n} {f} Mac"),
        format!("SPATIKEY1 PAIR {n} {f} 47612 Deck"),
        format!("SPATIKEY1 ASK {n} {f} De%4"),
        format!("SPATIKEY1 ASK {n} {f} De%G1"),
        format!("SPATIKEY1 ASK {n} {f} %0A"),
        format!("SPATIKEY1 ASK {n} {f} %FF%FE"),
        format!("SPATIKEY1 ASK {n} {f} Deck/1"),
        format!("SPATIKEY1 ASK {n} {f} {}", "a".repeat(lan::MAX_NAME + 1)),
        format!("SPATIKEY1 ASK {n} {f} {}", "%41".repeat(lan::MAX_NAME + 1)),
        format!("SPATIKEY1 ASK {n} {f} {}", "a".repeat(500)),
        format!("SPATIKEY1 ASK {n} {f} Déck"),
    ];
    for text in junk {
        assert_eq!(Beacon::decode(&text), None, "{text:?} read as a beacon");
    }
}

// MARK: what the same network means

#[test]
fn subnets_tunnels_and_mapped_addresses() {
    assert!(lan::in_subnet(ip("192.168.1.20"), ip("192.168.1.71"), 24));
    assert!(!lan::in_subnet(ip("192.168.10.28"), ip("192.168.1.71"), 24));
    assert!(lan::in_subnet(ip("::ffff:192.168.1.20"), ip("192.168.1.0"), 24), "a mapped address is the IPv4 one");
    assert!(lan::in_subnet(ip("192.168.1.20"), ip("::ffff:192.168.1.0"), 120), "and so is a mapped network");
    assert!(!lan::in_subnet(ip("192.168.1.20"), ip("::ffff:192.168.1.0"), 24), "whose prefix is IPv6's");
    assert!(lan::in_subnet(ip("2001:db8:1:2::9"), ip("2001:db8:1:2::71"), 64));
    assert!(!lan::in_subnet(ip("2001:db8:1:3::9"), ip("2001:db8:1:2::71"), 64));
    assert!(!lan::in_subnet(ip("192.168.1.20"), ip("2001:db8::"), 0), "families never mix");
    assert!(lan::in_subnet(ip("10.1.2.3"), ip("192.168.1.71"), 0), "a /0 holds everything, which is why none is a LAN");
    assert!(lan::in_subnet(ip("192.168.1.71"), ip("192.168.1.71"), 32));
    assert!(!lan::in_subnet(ip("192.168.1.72"), ip("192.168.1.71"), 32));
    assert!(!lan::in_subnet(ip("192.168.1.71"), ip("192.168.1.71"), 33), "no such prefix");
    assert!(!lan::in_subnet(ip("::1"), ip("::1"), 129));

    assert!(lan::is_tunnel(ip("100.121.56.71")), "this Mac's Tailscale address");
    assert!(lan::is_tunnel(ip("100.64.0.0")) && lan::is_tunnel(ip("100.127.255.255")));
    assert!(!lan::is_tunnel(ip("100.63.255.255")) && !lan::is_tunnel(ip("100.128.0.0")));
    assert!(lan::is_tunnel(ip("fd7a:115c:a1e0::c001:385d")));
    assert!(!lan::is_tunnel(ip("fd7a:115c:a1e1::1")));
    assert!(lan::is_tunnel(ip("::ffff:100.100.1.1")), "mapped, it is still the tunnel's");
    assert!(!lan::is_tunnel(ip("192.168.1.71")) && !lan::is_tunnel(ip("192.168.10.28")));

    assert_eq!(lan::canonical(ip("::ffff:192.168.1.71")), ip("192.168.1.71"));
    assert_eq!(lan::canonical(ip("::1")), ip("::1"));
    assert_eq!(lan::canonical(ip("192.168.1.71")), ip("192.168.1.71"));
}

fn en0() -> Lan {
    Lan { name: "en0".into(), index: 11, addr: ip("192.168.1.71"), prefix: 24, broadcast: Some(Ipv4Addr::new(192, 168, 1, 255)) }
}

#[test]
fn only_a_broadcast_from_inside_the_subnet_counts() {
    let lans = vec![en0()];
    let on = |source: &str, to: [u8; 4], index: u32| lan::heard_on(&lans, ip(source), Ipv4Addr::from(to), index).map(|l| l.name.clone());
    assert_eq!(on("192.168.1.50", [255, 255, 255, 255], 11).as_deref(), Some("en0"));
    assert_eq!(on("192.168.1.50", [192, 168, 1, 255], 11).as_deref(), Some("en0"));
    assert_eq!(on("192.168.1.50", [192, 168, 1, 71], 11), None, "sent to the Mac's own address, which anyone anywhere can do");
    assert_eq!(on("192.168.1.50", [255, 255, 255, 255], 25), None, "in by another interface (utun7's)");
    assert_eq!(on("192.168.10.28", [255, 255, 255, 255], 11), None, "from outside the subnet: almaserver standing in for the Beam Pro");
    assert_eq!(on("100.100.1.1", [255, 255, 255, 255], 11), None, "from a tunnel's address");
    assert_eq!(on("192.168.1.50", [192, 168, 10, 255], 11), None, "to another network's broadcast address");

    let wide = Lan { name: "en7".into(), index: 12, addr: ip("192.168.0.2"), prefix: 16, broadcast: Some(Ipv4Addr::new(192, 168, 255, 255)) };
    let both = vec![wide, en0()];
    assert_eq!(lan::lan_in(&both, ip("192.168.1.9")).map(|l| l.name.as_str()), Some("en0"), "the narrowest");
    assert_eq!(lan::lan_in(&both, ip("::ffff:192.168.7.9")).map(|l| l.name.as_str()), Some("en7"));
    assert_eq!(lan::lan_in(&both, ip("100.100.1.1")), None);
}

#[test]
fn a_device_is_near_only_where_and_while_it_was_heard() {
    let lans = [en0()];
    let device = Identity::new().unwrap().fingerprint();
    let other = Identity::new().unwrap().fingerprint();
    let heard = Heard::new();
    let now = Instant::now();
    let peer = ip("192.168.1.50");
    let mac = Some(ip("192.168.1.71"));
    assert!(heard.near(&lans, peer, mac, &device, now).is_err(), "never heard");
    heard.hear(peer, 11, device, now.checked_sub(Duration::from_secs(119)).expect("up two minutes"));
    assert!(heard.near(&lans, peer, mac, &device, now).is_ok(), "heard 119 s ago");
    assert!(
        heard.near(&lans, ip("::ffff:192.168.1.50"), Some(ip("::ffff:192.168.1.71")), &device, now).is_ok(),
        "the same, as a dual-stack socket reports it"
    );
    assert!(heard.near(&lans, peer, None, &device, now).is_ok(), "a local address the system did not give is not held against it");
    assert!(heard.near(&lans, peer, mac, &device, now + Duration::from_secs(2)).is_err(), "121 s after, it is forgotten");
    assert!(heard.near(&lans, peer, mac, &other, now).is_err(), "another certificate from that address");
    assert!(heard.near(&lans, ip("192.168.1.51"), mac, &device, now).is_err(), "that certificate from another address");
    let why = heard.near(&lans, peer, Some(ip("100.121.56.71")), &device, now).unwrap_err();
    assert!(why.contains("100.121.56.71"), "come in to the tunnel's address: {why}");
    assert!(heard.near(&[], peer, mac, &device, now).is_err(), "the Mac has left the network it heard it on");
    heard.hear(ip("100.100.1.1"), 11, device, now);
    assert!(heard.near(&lans, ip("100.100.1.1"), mac, &device, now).is_err(), "a tunnel's address, whatever was heard");
}

/// What this machine counts as its LANs. On this Mac (2026-10-29) that is en0's 192.168.1.71/24
/// and nothing of utun7, Tailscale's.
#[test]
fn this_machines_lans() {
    let lans = lan::interfaces();
    println!("this machine's LANs:");
    for lan in &lans {
        println!("  {} (index {}) {}/{} broadcast {:?}", lan.name, lan.index, lan.addr, lan.prefix, lan.broadcast);
    }
    for lan in &lans {
        assert!(!lan::is_tunnel(lan.addr), "{lan:?} is a tunnel's");
        assert!(!lan.addr.is_loopback() && !lan.addr.is_unspecified(), "{lan:?}");
        match lan.addr {
            IpAddr::V4(v4) => {
                assert!(!v4.is_link_local() && lan.broadcast.is_some() && (8..=32).contains(&lan.prefix), "{lan:?}")
            }
            IpAddr::V6(v6) => assert!(v6.segments()[0] & 0xffc0 != 0xfe80 && lan.broadcast.is_none(), "{lan:?}"),
        }
        assert_eq!(lan::index_of(&lan.name), Some(lan.index), "{lan:?}");
        assert!(!lan.name.starts_with("utun") && !lan.name.starts_with("tailscale") && !lan.name.starts_with("lo"), "{lan:?}");
        assert_eq!(lan::lan_of(lan.addr).map(|l| l.index), Some(lan.index), "its own address is on it");
    }
    assert_eq!(lan::lan_of(ip("100.121.56.71")), None);
}

// MARK: the Mac's side and a device's, on loopback

#[test]
fn the_mac_answers_only_a_broadcast_come_in_by_its_own_network() {
    let mac = Identity::new().unwrap();
    let device = Identity::new().unwrap().fingerprint().to_string();
    let answered = |network: Network| {
        let listening = remote::serve_with(Fake::new("Test Mac"), mac.clone(), network, any_ports()).expect("listens");
        let answer = beacon(listening.ports().discovery, &Beacon::Ask { nonce: nonce(), fp: device.clone(), name: "Test Deck".into() });
        (answer, listening.ports().ask)
    };
    match answered(loopback()) {
        (Some(Beacon::Here { nonce: n, fp, port, name }), ask) => {
            assert_eq!((n, fp, port, name), (nonce(), mac.fingerprint().to_string(), ask, "Test Mac".to_string()));
        }
        other => panic!("sent to what the rule calls the broadcast address, on its interface, it is answered: {other:?}"),
    }
    let elsewhere = Network::fixed(vec![loopback_lan(Some(Ipv4Addr::new(127, 255, 255, 255)))]);
    assert_eq!(answered(elsewhere).0, None, "with the broadcast address another, the same packet is one sent to the Mac's own address");
    let mut other_interface = loopback_lan(Some(Ipv4Addr::LOCALHOST));
    other_interface.index += 1000;
    assert_eq!(answered(Network::fixed(vec![other_interface])).0, None, "come in by another interface than the LAN's");
    let mut narrow = loopback_lan(Some(Ipv4Addr::LOCALHOST));
    narrow.addr = ip("127.0.0.2");
    narrow.prefix = 32;
    assert_eq!(answered(Network::fixed(vec![narrow])).0, None, "from outside the LAN's subnet");
    let here = Beacon::Here { nonce: nonce(), fp: device.clone(), port: 1, name: "Another Mac".into() };
    let listening = remote::serve_with(Fake::new("Test Mac"), mac.clone(), loopback(), any_ports()).unwrap();
    assert_eq!(beacon(listening.ports().discovery, &here), None, "a Mac does not answer another Mac");
}

#[test]
fn a_paired_device_on_the_same_network_is_answered() {
    let fake = Fake::new("Test Mac");
    let mac = Identity::new().unwrap();
    let device = Identity::new().unwrap();
    fake.allow_now(device.fingerprint(), "Test Deck");
    let listening = remote::serve_with(fake.clone(), mac.clone(), loopback(), any_ports()).expect("listens");
    let local = folder("answered");
    let mac_fp = mac.fingerprint().to_string();
    write_approvers(&local, &[Device { fp: mac_fp.clone(), name: "Test Mac".into(), added: 1 }]);
    let asker = Asker::with_network(device.clone(), local.clone(), "Test Deck".into(), loopback(), listening.ports().discovery);

    let found = asker.find(Duration::from_millis(800));
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!((found[0].fp.as_str(), found[0].name.as_str(), found[0].address.as_str(), found[0].paired), (mac_fp.as_str(), "Test Mac", "127.0.0.1", true));
    assert_eq!(asker.near(Duration::from_millis(800)), vec![Approver { fp: mac_fp.clone(), name: "Test Mac".into(), near: true }]);

    let mut lying = status(7);
    lying.who.device = "Somebody Else".into();
    let reply = asker.call(lying);
    assert_eq!(reply.id, 7);
    assert_eq!(reply.ok, Some(json!({ "op": "status" })), "{:?}", reply.err);
    let reply = asker.call(Envelope { id: 8, who: Who::default(), request: Request::Open });
    assert_eq!(reply.ok, Some(json!({ "op": "open" })), "{:?}", reply.err);
    let conn = {
        let seen = fake.seen.lock().unwrap();
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[0].1, Who { app: "SpatiMesh".into(), device: "Test Deck".into() }, "named as the Mac allowed it, not as it says");
        assert_eq!(seen[0].0, seen[1].0, "both requests on the one connection kept between them");
        seen[0].0
    };
    drop(asker);
    assert!(
        wait_for(Duration::from_secs(5), || fake.closed.lock().unwrap().contains(&conn)),
        "when the device goes, the Mac closes that connection's sessions"
    );
    drop(listening);
    let _ = std::fs::remove_dir_all(local);
}

#[test]
fn a_device_pairs_once_the_owner_and_the_wearer_both_say_yes() {
    let fake = Fake::new("Test Mac");
    fake.yes.store(true, Ordering::SeqCst);
    let mac = Identity::new().unwrap();
    let device = Identity::new().unwrap();
    let listening = remote::serve_with(fake.clone(), mac.clone(), loopback(), any_ports()).expect("listens");
    let local = folder("pairing");
    let asker = Asker::with_network(device.clone(), local.clone(), "Test Deck".into(), loopback(), listening.ports().discovery);
    let (mac_fp, device_fp) = (mac.fingerprint().to_string(), device.fingerprint().to_string());

    let found = asker.find(Duration::from_millis(800));
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(!found[0].paired);
    assert_eq!(refusal(&asker.call(status(1))), Some(Code::NoApprover), "with no Mac paired there is nobody to ask");

    let shown = remote::pairing_code(&device_fp, &mac_fp);
    assert_eq!(shown, remote::pairing_code(&mac_fp, &device_fp));
    assert_eq!(shown, transport::pairing_code(&device.fingerprint(), &mac.fingerprint()));
    assert_eq!(remote::pairing_code("not a fingerprint", &mac_fp), "");

    let answer = asker.pair(&mac_fp).expect("the owner said yes");
    assert_eq!(answer, PairAnswer { code: shown.clone(), paired: true });
    assert_eq!(
        *fake.asked.lock().unwrap(),
        vec![("Test Deck".to_string(), device_fp.clone(), shown.clone())],
        "the owner was asked with the digits the device shows"
    );
    assert!(fake.allowed().iter().any(|d| d.fp == device_fp && d.name == "Test Deck"), "the Mac wrote the device down");
    assert!(asker.approvers().is_empty(), "nothing kept on the device until its wearer says yes");
    asker.confirm(&mac_fp, true).expect("kept");
    let approvers = asker.approvers();
    assert_eq!(approvers.len(), 1);
    assert_eq!((approvers[0].fp.as_str(), approvers[0].name.as_str()), (mac_fp.as_str(), "Test Mac"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(local.join("approvers.json")).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "approvers.json is this user's alone");
    }

    let reply = asker.call(status(2));
    assert_eq!(reply.ok, Some(json!({ "op": "status" })), "{:?}", reply.err);

    // The window let one pairing through: asked again without a new PAIR broadcast, it is shut.
    let again = Envelope { id: 3, who: Who::default(), request: Request::Pair { name: "Test Deck".into() } };
    let reply = raw_request(&device, mac.fingerprint(), listening.ports().ask, &again).expect("an answer");
    assert_eq!(refusal(&reply), Some(Code::Denied), "{:?}", reply.err);
    assert_eq!(fake.asked.lock().unwrap().len(), 1, "and the owner was not asked again");
    drop(asker);
    drop(listening);
    let _ = std::fs::remove_dir_all(local);
}

#[test]
fn a_device_the_owner_turns_down_is_not_kept() {
    let fake = Fake::new("Test Mac");
    let mac = Identity::new().unwrap();
    let device = Identity::new().unwrap();
    let listening = remote::serve_with(fake.clone(), mac.clone(), loopback(), any_ports()).expect("listens");
    let local = folder("turned-down");
    let asker = Asker::with_network(device, local.clone(), "Test Deck".into(), loopback(), listening.ports().discovery);
    let mac_fp = mac.fingerprint().to_string();
    match asker.pair(&mac_fp) {
        Err(refusal) => assert_eq!(refusal.code, Code::Denied, "{refusal}"),
        Ok(answer) => panic!("paired although the owner said no: {answer:?}"),
    }
    assert_eq!(fake.asked.lock().unwrap().len(), 1);
    assert!(fake.allowed().is_empty());
    assert_eq!(asker.confirm(&mac_fp, true).map_err(|r| r.code), Err(Code::BadRequest), "nothing is waiting to be kept");
    assert!(asker.confirm(&mac_fp, false).is_ok());
    assert!(asker.approvers().is_empty());
    let unknown = Identity::new().unwrap().fingerprint().to_string();
    assert_eq!(asker.pair(&unknown).map_err(|r| r.code), Err(Code::NotReachable), "a Mac that does not answer");
    drop(asker);
    drop(listening);
    let _ = std::fs::remove_dir_all(local);
}

#[test]
fn a_device_the_mac_does_not_know_is_refused() {
    let fake = Fake::new("Test Mac");
    let mac = Identity::new().unwrap();
    let stranger = Identity::new().unwrap();
    let listening = remote::serve_with(fake.clone(), mac.clone(), loopback(), any_ports()).expect("listens");
    let local = folder("stranger");
    // It believes it is paired; the Mac has never heard of it.
    write_approvers(&local, &[Device { fp: mac.fingerprint().to_string(), name: "Test Mac".into(), added: 1 }]);
    let asker = Asker::with_network(stranger.clone(), local.clone(), "Stranger".into(), loopback(), listening.ports().discovery);
    let reply = asker.call(status(4));
    assert_eq!(refusal(&reply), Some(Code::NoApprover), "{:?}", reply.err);

    // Let through the handshake by a PAIR broadcast, it may pair and nothing else.
    let here = beacon(
        listening.ports().discovery,
        &Beacon::Pair { nonce: nonce(), fp: stranger.fingerprint().to_string(), name: "Stranger".into() },
    );
    assert!(matches!(here, Some(Beacon::Here { .. })), "a PAIR broadcast is answered: {here:?}");
    let reply = raw_request(&stranger, mac.fingerprint(), listening.ports().ask, &status(5)).expect("an answer");
    assert_eq!(refusal(&reply), Some(Code::NoApprover), "{:?}", reply.err);
    assert!(fake.ops().is_empty(), "the service was never asked");
    drop(asker);
    drop(listening);
    let _ = std::fs::remove_dir_all(local);
}

#[test]
fn a_paired_device_that_did_not_broadcast_is_refused() {
    let fake = Fake::new("Test Mac");
    let mac = Identity::new().unwrap();
    let device = Identity::new().unwrap();
    fake.allow_now(device.fingerprint(), "Test Deck");
    let listening = remote::serve_with(fake.clone(), mac.clone(), loopback(), any_ports()).expect("listens");
    let Ports { discovery, ask } = listening.ports();

    // Straight to the request port with no broadcast first, as a packet through a tunnel comes.
    let reply = raw_request(&device, mac.fingerprint(), ask, &status(6)).expect("an answer");
    assert_eq!(refusal(&reply), Some(Code::NotSameLan), "{:?}", reply.err);
    // A broadcast in another certificate's name does not count for this one.
    let other = Identity::new().unwrap().fingerprint().to_string();
    assert!(beacon(discovery, &Beacon::Ask { nonce: nonce(), fp: other, name: "Other".into() }).is_some());
    let reply = raw_request(&device, mac.fingerprint(), ask, &status(7)).expect("an answer");
    assert_eq!(refusal(&reply), Some(Code::NotSameLan), "{:?}", reply.err);
    assert!(fake.ops().is_empty(), "the service was never asked");
    // Its own does.
    let own = device.fingerprint().to_string();
    assert!(beacon(discovery, &Beacon::Ask { nonce: nonce(), fp: own, name: "Test Deck".into() }).is_some());
    let reply = raw_request(&device, mac.fingerprint(), ask, &status(8)).expect("an answer");
    assert_eq!(reply.ok, Some(json!({ "op": "status" })), "{:?}", reply.err);
    drop(listening);
}

/// A kept connection outlives the Mac's two minutes of memory, because the device keeps
/// broadcasting while it holds it. Takes two and a half minutes, so it is run by hand.
#[test]
#[ignore = "takes two and a half minutes; run by hand"]
fn a_kept_connection_outlives_the_macs_memory_of_one_broadcast() {
    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("spatikey_core=debug"))
        .is_test(true)
        .try_init();
    let fake = Fake::new("Test Mac");
    let mac = Identity::new().unwrap();
    let device = Identity::new().unwrap();
    fake.allow_now(device.fingerprint(), "Test Deck");
    let listening = remote::serve_with(fake.clone(), mac.clone(), loopback(), any_ports()).expect("listens");
    let local = folder("kept");
    write_approvers(&local, &[Device { fp: mac.fingerprint().to_string(), name: "Test Mac".into(), added: 1 }]);
    let asker = Asker::with_network(device, local.clone(), "Test Deck".into(), loopback(), listening.ports().discovery);
    assert_eq!(asker.call(status(1)).ok, Some(json!({ "op": "status" })));
    std::thread::sleep(remote::HEARD_FOR + Duration::from_secs(25));
    let reply = asker.call(status(2));
    assert_eq!(reply.ok, Some(json!({ "op": "status" })), "{:?}", reply.err);
    let seen = fake.seen.lock().unwrap();
    assert_eq!(seen[0].0, seen[1].0, "asked again on the connection it kept");
    drop(seen);
    drop(asker);
    drop(listening);
    let _ = std::fs::remove_dir_all(local);
}

// MARK: the real network

/// A Mac's side and a device's side in one process, the device's broadcast going out on this
/// machine's network and coming back in to the Mac's side. First it prints what the system says
/// of each broadcast that arrives (destination, interface), then it runs the real thing.
#[test]
#[ignore = "broadcasts on this machine's real network; run by hand"]
fn a_real_broadcast_on_this_network() {
    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("spatikey_core=debug"))
        .is_test(true)
        .try_init();
    let lans = lan::interfaces();
    println!("this machine's LANs: {lans:?}");
    let network = Network::system();

    // What arrives, as the system reports it.
    let listener = lan::listen(0).expect("a discovery socket");
    listener.set_read_timeout(Some(Duration::from_millis(300))).unwrap();
    let port = listener.local_addr().unwrap().port();
    let sender = std::net::UdpSocket::bind("0.0.0.0:0").unwrap();
    sender.set_broadcast(true).unwrap();
    for to in network.destinations(port) {
        match sender.send_to(b"SPATIKEY1 probe", to) {
            Ok(_) => println!("sent to {to}"),
            Err(e) => println!("could not send to {to}: {e} (os error {:?})", e.raw_os_error()),
        }
    }
    let mut buffer = [0u8; 2048];
    let until = Instant::now() + Duration::from_secs(1);
    while Instant::now() < until {
        match lan::receive(&listener, &mut buffer) {
            Ok(arrival) => {
                let counts = match (arrival.to, arrival.on) {
                    (Some(to), Some(on)) => lan::heard_on(&lans, IpAddr::V4(*arrival.from.ip()), to, on).map(|l| l.name.clone()),
                    _ => None,
                };
                println!(
                    "arrived from {} to {:?} in by interface {:?}: counts on {counts:?}",
                    arrival.from, arrival.to, arrival.on
                );
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(e) => println!("receiving failed: {e}"),
        }
    }
    drop(listener);

    // The real thing.
    let fake = Fake::new("Broadcast test Mac");
    fake.yes.store(true, Ordering::SeqCst);
    let mac = Identity::new().unwrap();
    let device = Identity::new().unwrap();
    let listening = remote::serve_with(fake.clone(), mac.clone(), Network::system(), any_ports()).expect("listens");
    let ports = listening.ports();
    println!("the Mac's side listens on {ports:?}");
    let local = folder("real");
    let asker = Asker::with_network(device, local.clone(), "Broadcast test device".into(), Network::system(), ports.discovery);
    let found = asker.find(Duration::from_secs(3));
    println!("found: {found:?}");
    let mac_fp = mac.fingerprint().to_string();
    assert!(found.iter().any(|f| f.fp == mac_fp), "the Mac's side did not answer the broadcast; what arrived is above");
    let answer = asker.pair(&mac_fp).expect("paired");
    println!("paired, both showing {}", answer.code);
    asker.confirm(&mac_fp, true).expect("kept");
    let reply = asker.call(status(1));
    println!("asked: ok {:?}, err {:?}", reply.ok, reply.err);
    assert_eq!(reply.ok, Some(json!({ "op": "status" })), "{:?}", reply.err);
    drop(asker);
    drop(listening);
    let _ = std::fs::remove_dir_all(local);
}
