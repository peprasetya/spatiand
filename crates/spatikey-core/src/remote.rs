//! The Deck and the Beam Pro asking the Mac. docs/spatikey.md, "The Deck and the Beam Pro ask
//! the Mac".
//!
//! Two halves. The Mac's, [`serve`], answers broadcasts on [`DISCOVERY_PORT`] and requests on
//! [`ASK_PORT`]; a device's, [`Asker`], broadcasts to find its Macs and asks them. The owner's
//! rule -- the Mac's sensor within reach, so the same network, and Tailscale is not that -- is
//! the Mac's to keep. It answers a device only if it heard that device's certificate broadcast
//! from the very address the request comes from, on one of its own networks, in the last two
//! minutes; it checks that for every request, not once; and it never takes the device's word for
//! where it is. What a device checks itself only saves a prompt that would be refused.
//!
//! Requests are [`crate::wire`]'s, one QUIC stream each, over the pinned certificates the rest
//! of Spatiand uses ([`spatiand_stream::transport`]), with the ALPN `spatikey/1` and connection
//! migration off: a connection stays on the address it was judged by.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use spatiand_stream::transport::{self, Fingerprint, Gate, Trust};
use spatiand_stream::Identity;
use zeroize::Zeroizing;

use crate::lan::{self, Arrival, Beacon, Lan, ASK_PORT, DISCOVERY_PORT};
use crate::service::{ConnId, Service};
use crate::wire::{self, Approver, Code, Device, Envelope, Found, PairAnswer, Refusal, Reply, Request, Who};

/// The ALPN both ends speak, so that a SpatiKey request never lands on a host's link, nor a link
/// on SpatiKey, though the Mac uses the same certificate for both.
pub const ALPN: &[u8] = b"spatikey/1";
/// How long a device's broadcast counts on the Mac.
pub const HEARD_FOR: Duration = Duration::from_secs(120);
/// How long a `PAIR` broadcast lets that one certificate through the Mac's handshake.
pub const PAIRING_WINDOW: Duration = Duration::from_secs(60);
/// The longest a device waits for an answer: a person may be asked first.
pub const ANSWER_WAIT: Duration = Duration::from_secs(120);
/// How often a device keeping a connection broadcasts again: well inside [`HEARD_FOR`], so that
/// one lost broadcast does not make the Mac forget it.
const ASK_EVERY: Duration = Duration::from_secs(45);
/// A kept connection whose Mac last answered a broadcast longer ago than this is heard again
/// before it carries a request.
const STALE: Duration = Duration::from_secs(60);
/// How long a device listens for its Mac before a request.
const FIND_WAIT: Duration = Duration::from_millis(1500);
/// How long it listens for the Mac it is pairing with.
const PAIR_FIND_WAIT: Duration = Duration::from_secs(3);
/// Broadcasts on Wi-Fi go out unacknowledged at the slowest rate and are lost now and then, so
/// each is said more than once, this far apart.
const RESEND: Duration = Duration::from_millis(500);
const SENDS: u32 = 3;
const CONNECT_WAIT: Duration = Duration::from_secs(10);
/// How long the Mac waits for a request to arrive whole once its stream is open.
const REQUEST_WAIT: Duration = Duration::from_secs(30);
/// The name a device connects to. Never checked -- a fingerprint is the identity -- but a
/// connection has to have one.
const SERVER_NAME: &str = "spatiand";
/// The most broadcasts the Mac keeps in mind, so that a flood of made-up ones costs it nothing.
const MAX_HEARD: usize = 1024;
/// The most pairing windows open at once.
const MAX_WINDOWS: usize = 8;
/// Where a device keeps its Macs, in `local/spatikey`.
const APPROVERS: &str = "approvers.json";
/// What a device adds when the Mac says it is not on its network: the usual reason, and the
/// two cures.
const TUNNEL_HINT: &str = "If Tailscale is on here, it may be carrying this device's packets to the Mac: \
                           exclude Spatiand from Tailscale, or turn off its subnet routes.";

/// Where the two halves meet: [`DISCOVERY_PORT`] and [`ASK_PORT`], or others in tests (0 for
/// whatever the system gives).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ports {
    pub discovery: u16,
    pub ask: u16,
}

impl Default for Ports {
    fn default() -> Ports {
        Ports { discovery: DISCOVERY_PORT, ask: ASK_PORT }
    }
}

/// Which networks this machine counts as its own, and where a device's broadcast goes.
///
/// [`Network::system`] is the rule: the interfaces as [`lan::interfaces`] finds them at that
/// moment, and broadcasts to 255.255.255.255 and to every LAN's own broadcast address. A test on
/// loopback could never pass it -- loopback is among the first things it refuses -- so a test
/// gives [`Network::fixed`] instead.
#[derive(Clone)]
pub struct Network {
    fixed: Option<Arc<Vec<Lan>>>,
}

impl Network {
    pub fn system() -> Network {
        Network { fixed: None }
    }

    /// Exactly these LANs, and broadcasts only to their own broadcast addresses: never to
    /// 255.255.255.255, which would go out on the real network.
    pub fn fixed(lans: Vec<Lan>) -> Network {
        Network { fixed: Some(Arc::new(lans)) }
    }

    pub fn lans(&self) -> Vec<Lan> {
        match &self.fixed {
            Some(lans) => lans.as_ref().clone(),
            None => lan::interfaces(),
        }
    }

    /// Where a broadcast to `port` is sent.
    pub fn destinations(&self, port: u16) -> Vec<SocketAddr> {
        let mut out = Vec::new();
        if self.fixed.is_none() {
            out.push(SocketAddr::new(Ipv4Addr::BROADCAST.into(), port));
        }
        for lan in self.lans() {
            if let Some(broadcast) = lan.broadcast {
                let to = SocketAddr::new(broadcast.into(), port);
                if !out.contains(&to) {
                    out.push(to);
                }
            }
        }
        out
    }
}

impl std::fmt::Debug for Network {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.fixed {
            None => write!(f, "Network::system"),
            Some(lans) => f.debug_tuple("Network::fixed").field(lans).finish(),
        }
    }
}

/// The six digits both ends show while a device pairs with a Mac (`transport::pairing_code`),
/// from the two fingerprints in hex, in either order. Empty if either is not a fingerprint.
pub fn pairing_code(mine: &str, theirs: &str) -> String {
    match (mine.parse::<Fingerprint>(), theirs.parse::<Fingerprint>()) {
        (Ok(mine), Ok(theirs)) => transport::pairing_code(&mine, &theirs),
        _ => String::new(),
    }
}

// MARK: the Mac's side

/// What the Mac's side needs of the service. [`Service`] is the real one; a test stands in for
/// it, because what is tested here is who gets through, not what they are answered.
pub trait Answers: Send + Sync {
    /// This Mac's name, as its answers to broadcasts give it.
    fn name(&self) -> String;
    /// The devices allowed to ask.
    fn allowed(&self) -> Vec<Device>;
    /// Let a device ask from now on.
    fn allow(&self, device: Device) -> Result<(), String>;
    /// The owner's yes or no to a device that wants to ask, with the digits both ends show.
    /// Blocks until the owner answers.
    fn allow_device(&self, name: &str, fp: &str, code: &str) -> bool;
    /// Answer one request. Blocks while a person is asked.
    fn handle(&self, conn: ConnId, env: Envelope) -> Reply;
    /// The connection has gone: its sessions close.
    fn closed(&self, conn: ConnId);
}

impl Answers for Service {
    fn name(&self) -> String {
        self.me().name.clone()
    }

    fn allowed(&self) -> Vec<Device> {
        self.may_ask()
    }

    fn allow(&self, device: Device) -> Result<(), String> {
        Service::allow(self, device)
    }

    fn allow_device(&self, name: &str, fp: &str, code: &str) -> bool {
        self.guard().is_some_and(|guard| guard.allow_device(name, fp, code))
    }

    fn handle(&self, conn: ConnId, env: Envelope) -> Reply {
        Service::handle(self, conn, env)
    }

    fn closed(&self, conn: ConnId) {
        Service::closed(self, conn)
    }
}

/// What the Mac remembers of devices' broadcasts -- which certificate, from which address, come
/// in by which interface, when -- and the test every request must pass against it.
#[derive(Default)]
pub struct Heard {
    seen: Mutex<HashMap<(IpAddr, u32, Fingerprint), Instant>>,
}

impl Heard {
    pub fn new() -> Heard {
        Heard::default()
    }

    /// A broadcast from the device `fp` at `ip`, come in by the interface `index`, at `at`.
    pub fn hear(&self, ip: IpAddr, index: u32, fp: Fingerprint, at: Instant) {
        let mut seen = lock(&self.seen);
        seen.retain(|_, when| at.saturating_duration_since(*when) <= HEARD_FOR);
        if seen.len() >= MAX_HEARD {
            if let Some(oldest) = seen.iter().min_by_key(|(_, when)| **when).map(|(key, _)| *key) {
                seen.remove(&oldest);
            }
        }
        seen.insert((lan::canonical(ip), index, fp), at);
    }

    /// Is the device `fp`, connected from `peer` to this machine's address `local` (when the
    /// system says it), near at `now`?
    ///
    /// It is when it broadcast from that very address within [`HEARD_FOR`], on an interface
    /// whose subnet still holds it, and -- when the connection's local address is known -- the
    /// connection came in to that same interface's address rather than, say, the tunnel's. A
    /// refusal says why, for the device to show.
    pub fn near(&self, lans: &[Lan], peer: IpAddr, local: Option<IpAddr>, fp: &Fingerprint, now: Instant) -> Result<(), String> {
        let peer = lan::canonical(peer);
        if lan::is_tunnel(peer) {
            return Err(format!(
                "the request came from {peer}, a Tailscale address, and SpatiKey answers only on this Mac's own network"
            ));
        }
        let local = local.map(lan::canonical);
        let seen = lock(&self.seen);
        let mut why = None;
        for (&(ip, index, who), &when) in seen.iter() {
            if ip != peer || who != *fp || now.saturating_duration_since(when) > HEARD_FOR {
                continue;
            }
            if !lans.iter().any(|lan| lan.index == index && lan::in_subnet(peer, lan.addr, lan.prefix)) {
                why = Some(format!("{peer} is no longer on the network this Mac heard it on"));
                continue;
            }
            if let Some(local) = local {
                if !lans.iter().any(|lan| lan.index == index && lan.addr == local) {
                    why = Some(format!(
                        "the request came in to {local}, not to this Mac's address on the network it heard the device on"
                    ));
                    continue;
                }
            }
            return Ok(());
        }
        Err(why.unwrap_or_else(|| {
            format!("this Mac has not heard this device broadcasting from {peer} on its own network in the last two minutes")
        }))
    }

    fn forget_old(&self, now: Instant) {
        lock(&self.seen).retain(|_, when| now.saturating_duration_since(*when) <= HEARD_FOR);
    }
}

/// The Mac's side, running: answering broadcasts on [`crate::lan::DISCOVERY_PORT`] and requests on
/// [`crate::lan::ASK_PORT`]. Dropping it stops both.
pub struct Listening {
    stop: Option<std::sync::mpsc::Sender<()>>,
    ports: Ports,
    endpoint: Option<quinn::Endpoint>,
    runtime: Option<tokio::runtime::Runtime>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Listening {
    /// The ports it took: the ones asked for, or what the system gave for 0.
    pub fn ports(&self) -> Ports {
        self.ports
    }
}

impl Drop for Listening {
    fn drop(&mut self) {
        // The discovery thread sees the channel close when it next wakes, within a quarter second.
        self.stop.take();
        let endpoint = self.endpoint.take();
        if let Some(endpoint) = &endpoint {
            endpoint.close(0u32.into(), b"SpatiKey stopped");
        }
        if let Some(runtime) = self.runtime.take() {
            if tokio::runtime::Handle::try_current().is_err() {
                // A moment for the close to reach the devices, so that they stop waiting now
                // rather than when the connection times out.
                runtime.block_on(async move {
                    if let Some(endpoint) = endpoint {
                        let _ = tokio::time::timeout(Duration::from_millis(200), endpoint.wait_idle()).await;
                    }
                });
                runtime.shutdown_timeout(Duration::from_secs(1));
            } else {
                runtime.shutdown_background();
            }
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Start the Mac's side for `service` with this Mac's link identity. Requests are answered only
/// for devices in `service.allowed()`, only from a LAN address heard broadcasting within two
/// minutes, each checked again per request; a device that broadcast `PAIR` may connect once,
/// for a minute, to ask `pair`.
pub fn serve(service: Arc<Service>, identity: Identity) -> Result<Listening, String> {
    if !service.is_approver() {
        return Err("only a Mac that opens the vault itself answers other devices".into());
    }
    serve_with(service, identity, Network::system(), Ports::default())
}

/// [`serve`] for any [`Answers`], by another [`Network`] rule, on other ports: what tests drive.
pub fn serve_with(answers: Arc<dyn Answers>, identity: Identity, network: Network, ports: Ports) -> Result<Listening, String> {
    let socket = lan::listen(ports.discovery)
        .map_err(|e| format!("could not listen for broadcasts on UDP {}: {e}", ports.discovery))?;
    socket
        .set_read_timeout(Some(Duration::from_millis(250)))
        .map_err(|e| format!("could not set the discovery socket up: {e}"))?;
    let discovery = socket.local_addr().map_err(|e| format!("the discovery socket has no address: {e}"))?.port();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("spatikey-mac")
        .enable_all()
        .build()
        .map_err(|e| format!("could not start the network runtime: {e}"))?;
    let gate = Arc::new(Gate::new(Vec::new()));
    let endpoint = {
        let _inside = runtime.enter();
        let listen = |ip: IpAddr| {
            transport::server_with(SocketAddr::new(ip, ports.ask), &identity, Trust::Gate(gate.clone()), &[ALPN], false)
        };
        listen(Ipv6Addr::UNSPECIFIED.into()).or_else(|_| listen(Ipv4Addr::UNSPECIFIED.into()))?
    };
    let ask = endpoint.local_addr().map_err(|e| format!("the request socket has no address: {e}"))?.port();
    let mac = Arc::new(Mac {
        answers,
        network,
        me: identity.fingerprint(),
        ask_port: ask,
        gate,
        heard: Heard::new(),
        windows: Mutex::new(Vec::new()),
        pairing: AtomicBool::new(false),
        gate_refreshed: Mutex::new(None),
    });
    mac.refresh_gate(true);
    runtime.spawn(take_requests(mac.clone(), endpoint.clone()));
    runtime.spawn(tidy(mac.clone()));
    let (stop, stopped) = mpsc::channel();
    // Made before the thread, so that if the thread cannot start, what did start is stopped
    // the one way it always is.
    let mut listening = Listening {
        stop: Some(stop),
        ports: Ports { discovery, ask },
        endpoint: Some(endpoint),
        runtime: Some(runtime),
        thread: None,
    };
    let thread = std::thread::Builder::new()
        .name("spatikey-discovery".into())
        .spawn(move || hear_broadcasts(&mac, &socket, &stopped))
        .map_err(|e| format!("could not start the discovery thread: {e}"))?;
    listening.thread = Some(thread);
    log::info!("SpatiKey answers devices on this network: broadcasts on UDP {discovery}, requests on UDP {ask}");
    Ok(listening)
}

/// The Mac's side, shared by the discovery thread and the connections.
struct Mac {
    answers: Arc<dyn Answers>,
    network: Network,
    me: Fingerprint,
    ask_port: u16,
    gate: Arc<Gate>,
    heard: Heard,
    windows: Mutex<Vec<Window>>,
    /// The owner is asked about one device at a time: asked about two at once, nobody could tell
    /// whose digits were whose.
    pairing: AtomicBool,
    gate_refreshed: Mutex<Option<Instant>>,
}

/// A `PAIR` broadcast: that certificate may pass the handshake, from that address, for a minute.
struct Window {
    fp: Fingerprint,
    ip: IpAddr,
    opened: Instant,
}

fn hear_broadcasts(mac: &Mac, socket: &UdpSocket, stop: &mpsc::Receiver<()>) {
    let mut buffer = [0u8; 2048];
    while matches!(stop.try_recv(), Err(mpsc::TryRecvError::Empty)) {
        match lan::receive(socket, &mut buffer) {
            Ok(arrival) => mac.beacon(socket, &arrival, &buffer[..arrival.len]),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::Interrupted
                        | std::io::ErrorKind::InvalidData
                ) => {}
            Err(e) => {
                log::warn!("the discovery socket failed: {e}");
                std::thread::sleep(Duration::from_millis(500));
            }
        }
    }
}

impl Mac {
    fn beacon(&self, socket: &UdpSocket, arrival: &Arrival, bytes: &[u8]) {
        let Some(beacon) = std::str::from_utf8(bytes).ok().and_then(Beacon::decode) else {
            return;
        };
        let (nonce, fp, name, pair) = match beacon {
            Beacon::Ask { nonce, fp, name } => (nonce, fp, name, false),
            Beacon::Pair { nonce, fp, name } => (nonce, fp, name, true),
            Beacon::Here { .. } => return,
        };
        let Ok(fp) = fp.parse::<Fingerprint>() else {
            return;
        };
        if fp == self.me {
            return;
        }
        let source = IpAddr::V4(*arrival.from.ip());
        let (Some(to), Some(on)) = (arrival.to, arrival.on) else {
            log::debug!("a beacon from {} came without its destination or interface; not counted", arrival.from);
            return;
        };
        let lans = self.network.lans();
        let Some(lan) = lan::heard_on(&lans, source, to, on) else {
            log::debug!(
                "a beacon from {} to {to}, in by interface {on}, is not a broadcast on this Mac's network; not counted",
                arrival.from
            );
            return;
        };
        let now = Instant::now();
        self.heard.hear(source, lan.index, fp, now);
        log::debug!("heard {} ({}) broadcasting from {source} on {}", clean_name(&name), fp.short(), lan.name);
        if pair && self.open_window(fp, source, now) {
            log::info!("{} ({}) is about to ask to pair, from {source} on {}", clean_name(&name), fp.short(), lan.name);
        }
        self.refresh_gate(pair);
        let here = Beacon::Here { nonce, fp: self.me.to_string(), port: self.ask_port, name: self.answers.name() }.encode();
        if let Err(e) = socket.send_to(here.as_bytes(), arrival.from) {
            log::info!("could not answer {}: {e}", arrival.from);
        }
    }

    /// Open the window for `fp` from `ip`, or start its minute again. True when none was open
    /// for them: a device says `PAIR` to every broadcast address, more than once, and the log
    /// need not say so each time.
    fn open_window(&self, fp: Fingerprint, ip: IpAddr, now: Instant) -> bool {
        let mut windows = lock(&self.windows);
        windows.retain(|w| now.saturating_duration_since(w.opened) < PAIRING_WINDOW);
        let new = !windows.iter().any(|w| w.fp == fp && w.ip == ip);
        windows.retain(|w| w.fp != fp);
        if windows.len() >= MAX_WINDOWS {
            windows.remove(0);
        }
        windows.push(Window { fp, ip, opened: now });
        new
    }

    /// Close the window `fp` broadcast from `ip`, if there is one still open: it lets one pairing
    /// through, not two.
    fn take_window(&self, fp: &Fingerprint, ip: IpAddr) -> bool {
        let now = Instant::now();
        let mut windows = lock(&self.windows);
        windows.retain(|w| now.saturating_duration_since(w.opened) < PAIRING_WINDOW);
        match windows.iter().position(|w| w.fp == *fp && w.ip == ip) {
            Some(at) => {
                windows.remove(at);
                true
            }
            None => false,
        }
    }

    /// Let through the handshake the devices allowed to ask and those in an open window, and no
    /// others. It is a first filter only -- what a device may do is decided again after the
    /// handshake, request by request -- so it is refreshed when a broadcast comes (before the
    /// device connects), at most once a second unless `force`, and every ten seconds.
    fn refresh_gate(&self, force: bool) {
        {
            let mut last = lock(&self.gate_refreshed);
            if !force && last.is_some_and(|at| at.elapsed() < Duration::from_secs(1)) {
                return;
            }
            *last = Some(Instant::now());
        }
        let mut admitted: Vec<Fingerprint> = self.answers.allowed().iter().filter_map(|d| d.fp.parse().ok()).collect();
        let now = Instant::now();
        {
            let mut windows = lock(&self.windows);
            windows.retain(|w| now.saturating_duration_since(w.opened) < PAIRING_WINDOW);
            admitted.extend(windows.iter().map(|w| w.fp));
        }
        self.gate.set_paired(admitted);
    }

    async fn connection(self: Arc<Self>, connection: quinn::Connection) {
        let Some(fp) = transport::peer_fingerprint(&connection) else {
            connection.close(1u32.into(), b"no certificate");
            return;
        };
        if !speaks_spatikey(&connection) {
            connection.close(1u32.into(), b"not spatikey/1");
            return;
        }
        let peer = connection.remote_address();
        log::debug!("{} connected from {peer} to {:?}", fp.short(), connection.local_ip());
        if let Err(why) = self.heard.near(&self.network.lans(), peer.ip(), connection.local_ip(), &fp, Instant::now()) {
            // Still served, request by request, so that the device hears why; nothing gets past it.
            log::info!("{} connected from {peer}, which is not near: {why}", fp.short());
        }
        let conn = crate::service::next_conn();
        let mut requests = tokio::task::JoinSet::new();
        loop {
            match connection.accept_bi().await {
                Ok((send, recv)) => {
                    requests.spawn(self.clone().request(connection.clone(), conn, fp, send, recv));
                }
                Err(e) => {
                    log::debug!("{} left: {e}", fp.short());
                    break;
                }
            }
            while requests.try_join_next().is_some() {}
        }
        // Its sessions close once nothing is still being answered for it.
        while requests.join_next().await.is_some() {}
        let mac = self.clone();
        let _ = tokio::task::spawn_blocking(move || mac.answers.closed(conn)).await;
    }

    async fn request(
        self: Arc<Self>,
        connection: quinn::Connection,
        conn: ConnId,
        fp: Fingerprint,
        mut send: quinn::SendStream,
        mut recv: quinn::RecvStream,
    ) {
        let bytes = match tokio::time::timeout(REQUEST_WAIT, read_message(&mut recv)).await {
            Ok(Ok(bytes)) => bytes,
            Ok(Err(e)) => {
                log::debug!("a request from {} could not be read: {e}", fp.short());
                return;
            }
            Err(_) => {
                log::debug!("a request from {} did not arrive", fp.short());
                return;
            }
        };
        let reply = self.answer(&connection, conn, fp, bytes).await;
        let mut framed = Zeroizing::new(Vec::new());
        let written = serde_json::to_vec(&reply)
            .map(Zeroizing::new)
            .map_err(|e| e.to_string())
            .and_then(|json| wire::write_frame(&mut *framed, &json).map_err(|e| e.to_string()));
        if let Err(e) = written {
            log::warn!("could not write an answer for {}: {e}", fp.short());
            return;
        }
        if let Err(e) = send.write_all(&framed).await {
            log::debug!("an answer to {} did not go: {e}", fp.short());
        }
        let _ = send.finish();
    }

    async fn answer(self: &Arc<Self>, connection: &quinn::Connection, conn: ConnId, fp: Fingerprint, bytes: Zeroizing<Vec<u8>>) -> Reply {
        let mut env: Envelope = match serde_json::from_slice(&bytes) {
            Ok(env) => env,
            // Where it went wrong, but not what was there: it may be a password.
            Err(e) => {
                return Reply::err(
                    id_of(&bytes),
                    Refusal::new(Code::BadRequest, format!("not a request (line {}, column {})", e.line(), e.column())),
                )
            }
        };
        drop(bytes);
        let id = env.id;
        let peer = connection.remote_address();
        if let Err(why) = self.heard.near(&self.network.lans(), peer.ip(), connection.local_ip(), &fp, Instant::now()) {
            log::info!("refused a request from {} at {peer}: {why}", fp.short());
            return Reply::err(id, Refusal::new(Code::NotSameLan, why));
        }
        if let Request::Pair { name } = &env.request {
            let name = name.clone();
            return self.pair(peer.ip(), fp, id, name).await;
        }
        let mac = self.clone();
        tokio::task::spawn_blocking(move || {
            let allowed = mac.answers.allowed();
            let Some(device) = allowed.into_iter().find(|d| d.fp.parse::<Fingerprint>().ok() == Some(fp)) else {
                return Reply::err(id, Refusal::new(Code::NoApprover, "this device is not paired with this Mac: pair it again"));
            };
            // The name the owner allowed, never the one the device gives itself now.
            env.who.device = device.name;
            env.who.app = clean_name(&env.who.app);
            mac.answers.handle(conn, env)
        })
        .await
        .unwrap_or_else(|e| Reply::err(id, Refusal::new(Code::Failed, format!("the service failed: {e}"))))
    }

    async fn pair(self: &Arc<Self>, peer: IpAddr, fp: Fingerprint, id: u64, name: String) -> Reply {
        if !self.take_window(&fp, lan::canonical(peer)) {
            return Reply::err(
                id,
                Refusal::new(Code::Denied, "this Mac is not expecting this device to pair: start the pairing again"),
            );
        }
        if self.pairing.swap(true, Ordering::SeqCst) {
            return Reply::err(id, Refusal::new(Code::Denied, "another device is pairing with this Mac: try again in a minute"));
        }
        let code = transport::pairing_code(&self.me, &fp);
        let shown = match clean_name(&name) {
            name if name.is_empty() => format!("a device ({})", fp.short()),
            name => name,
        };
        log::info!("{shown} ({}) asks to use SpatiKey on this Mac; asking the owner", fp.short());
        let mac = self.clone();
        tokio::task::spawn_blocking(move || {
            // The window is closed: the gate stops letting that certificate through.
            mac.refresh_gate(true);
            let fp_text = fp.to_string();
            let reply = if !mac.answers.allow_device(&shown, &fp_text, &code) {
                Reply::err(id, Refusal::new(Code::Denied, "the Mac's owner said no"))
            } else {
                match mac.answers.allow(Device { fp: fp_text, name: shown, added: crate::now_ms() }) {
                    Ok(()) => Reply::ok(id, PairAnswer { code, paired: true }),
                    Err(e) => Reply::err(id, Refusal::new(Code::Failed, format!("could not write the device down: {e}"))),
                }
            };
            mac.pairing.store(false, Ordering::SeqCst);
            mac.refresh_gate(true);
            reply
        })
        .await
        .unwrap_or_else(|e| {
            self.pairing.store(false, Ordering::SeqCst);
            Reply::err(id, Refusal::new(Code::Failed, format!("asking the owner failed: {e}")))
        })
    }
}

async fn take_requests(mac: Arc<Mac>, endpoint: quinn::Endpoint) {
    while let Some(incoming) = endpoint.accept().await {
        let mac = mac.clone();
        tokio::spawn(async move {
            match incoming.await {
                Ok(connection) => mac.connection(connection).await,
                Err(e) => log::info!("a device did not get as far as connecting: {e}"),
            }
        });
    }
}

/// Windows close and forgotten devices leave the gate even when no broadcast comes.
async fn tidy(mac: Arc<Mac>) {
    let mut every = tokio::time::interval(Duration::from_secs(10));
    loop {
        every.tick().await;
        let mac = mac.clone();
        let _ = tokio::task::spawn_blocking(move || {
            mac.refresh_gate(true);
            mac.heard.forget_old(Instant::now());
        })
        .await;
    }
}

// MARK: a device's side

/// A device's side: finding its Macs and asking them.
pub struct Asker {
    identity: Identity,
    local: PathBuf,
    name: String,
    beacons: Beacons,
    /// The connection kept to a Mac between requests: one touch can cover several, and a session
    /// belongs to the connection that opened it.
    link: Mutex<Option<Arc<Link>>>,
    /// One finding-and-connecting at a time, so that two requests at once share one connection.
    connecting: tokio::sync::Mutex<()>,
    /// Macs whose owner said yes, waiting for this device's wearer: fingerprint to name.
    pending: Mutex<HashMap<Fingerprint, String>>,
    runtime: OnceLock<Result<tokio::runtime::Runtime, String>>,
    /// The hosts this device has paired with: those that are Macs need no second pairing.
    hosts: Mutex<Option<HostList>>,
}

/// What [`Asker::also_ask`] is given: read each time, as hosts come and go.
pub type HostList = Box<dyn Fn() -> Vec<Device> + Send + Sync>;

impl Asker {
    /// `local` is `local/spatikey`, where `approvers.json` is kept; `name` is this device's.
    pub fn new(identity: Identity, local: PathBuf, name: String) -> Asker {
        Asker::with_network(identity, local, name, Network::system(), DISCOVERY_PORT)
    }

    /// [`Asker::new`] by another [`Network`] rule, broadcasting to another port: what tests drive.
    pub fn with_network(identity: Identity, local: PathBuf, name: String, network: Network, discovery: u16) -> Asker {
        let beacons = Beacons { network, port: discovery, fp: identity.fingerprint().to_string(), name: name.clone() };
        Asker {
            identity,
            local,
            name,
            beacons,
            link: Mutex::new(None),
            connecting: tokio::sync::Mutex::new(()),
            pending: Mutex::new(HashMap::new()),
            runtime: OnceLock::new(),
            hosts: Mutex::new(None),
        }
    }

    /// Ask the Macs among the hosts this device is paired with, as well as those in its own list.
    /// Only a Mac answers a broadcast, and each is pinned to the certificate paired with.
    pub fn also_ask(&self, hosts: HostList) {
        *lock(&self.hosts) = Some(hosts);
    }

    /// The Macs this device can ask: those paired with SpatiKey, and its hosts.
    pub fn approvers(&self) -> Vec<Device> {
        let mut devices = self.paired_approvers();
        if let Some(hosts) = lock(&self.hosts).as_ref() {
            for host in hosts() {
                if !devices.iter().any(|d| same_fp(&d.fp, &host.fp)) {
                    devices.push(host);
                }
            }
        }
        devices
    }

    /// The Macs this device has paired with SpatiKey.
    fn paired_approvers(&self) -> Vec<Device> {
        let path = self.local.join(APPROVERS);
        match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                log::warn!("{} is not a list of Macs: {e}", path.display());
                Vec::new()
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => {
                log::warn!("could not read {}: {e}", path.display());
                Vec::new()
            }
        }
    }

    /// The paired Macs, each with whether it answers on this network right now.
    pub fn near(&self, wait: Duration) -> Vec<Approver> {
        let approvers = self.approvers();
        if approvers.is_empty() {
            return Vec::new();
        }
        let found = self.find(wait);
        let paired = self.paired_approvers();
        approvers
            .into_iter()
            .map(|d| {
                let near = found.iter().any(|f| same_fp(&f.fp, &d.fp));
                Approver { fp: d.fp, name: d.name, near }
            })
            // A host that is not here may be no Mac at all: not one to say could approve.
            .filter(|a| a.near || paired.iter().any(|d| same_fp(&d.fp, &a.fp)))
            .collect()
    }

    /// Broadcast and collect the Macs that answer within `wait`.
    pub fn find(&self, wait: Duration) -> Vec<Found> {
        let approvers = self.approvers();
        let answered = self.run(async { self.beacons.discover(false, wait, |_| false).await.0 }).unwrap_or_default();
        answered
            .into_iter()
            .map(|a| {
                let fp = a.fp.to_string();
                let paired = approvers.iter().any(|d| same_fp(&d.fp, &fp));
                Found { fp, name: a.name, address: a.ip.to_string(), paired }
            })
            .collect()
    }

    /// Send `env` to a paired Mac on this network and wait for its answer (a person may be
    /// asked: up to two minutes). `not_reachable` if none answers the broadcast, `no_approver`
    /// with none paired.
    pub fn call(&self, env: Envelope) -> Reply {
        let id = env.id;
        let paired: Vec<Fingerprint> = self.approvers().iter().filter_map(|d| d.fp.parse().ok()).collect();
        if paired.is_empty() {
            return Reply::err(id, Refusal::new(Code::NoApprover, "this device is not paired with a Mac yet: pair it with one in SpatiKey"));
        }
        let framed = match frame(&env) {
            Ok(framed) => framed,
            Err(refusal) => return Reply::err(id, refusal),
        };
        drop(env);
        match self.run(self.ask(&paired, &framed, id)) {
            Ok(Ok(reply)) => reply,
            Ok(Err(refusal)) | Err(refusal) => Reply::err(id, refusal),
        }
    }

    /// Ask the Mac `fp` (found by [`Asker::find`]) to let this device in. Returns once its owner
    /// has answered, with the six digits; nothing is kept until [`Asker::confirm`].
    pub fn pair(&self, fp: &str) -> Result<PairAnswer, Refusal> {
        let mac: Fingerprint = fp.parse().map_err(|e: String| Refusal::new(Code::BadRequest, e))?;
        let mine = self.identity.fingerprint();
        if mac == mine {
            return Err(Refusal::new(Code::BadRequest, "a device cannot pair with itself"));
        }
        let framed = frame(&Envelope { id: 1, who: Who::default(), request: Request::Pair { name: self.name.clone() } })?;
        let (reply, name) = self.run(async {
            // The broadcast that opens the Mac's window for this certificate, and finds the Mac.
            let (answered, failure) = self.beacons.discover(true, PAIR_FIND_WAIT, |a| a.fp == mac).await;
            let Some(found) = answered.into_iter().find(|a| a.fp == mac) else {
                return Err(not_found(failure, "that Mac did not answer on this network"));
            };
            let link = self.connect(&found).await?;
            let result = exchange(&link.connection, &framed, ANSWER_WAIT).await;
            link.connection.close(0u32.into(), b"done");
            let _ = tokio::time::timeout(Duration::from_millis(100), link.endpoint.wait_idle()).await;
            Ok((result.map_err(Lost::refusal)?, found.name))
        })??;
        let answer: PairAnswer = serde_json::from_value(reply.into_result()?)
            .map_err(|e| Refusal::new(Code::Failed, format!("an answer this device cannot read: {e}")))?;
        if !answer.paired {
            return Err(Refusal::new(Code::Denied, "the Mac did not let this device in"));
        }
        let code = transport::pairing_code(&mine, &mac);
        if answer.code != code {
            return Err(Refusal::new(
                Code::Failed,
                format!("the Mac worked out other digits ({}) than this device ({code}): do not pair", answer.code),
            ));
        }
        lock(&self.pending).insert(mac, name);
        Ok(PairAnswer { code, paired: true })
    }

    /// The wearer saw the same digits: keep the Mac in `approvers.json` (or, `ok` false, forget
    /// the attempt).
    pub fn confirm(&self, fp: &str, ok: bool) -> Result<(), Refusal> {
        let mac: Fingerprint = fp.parse().map_err(|e: String| Refusal::new(Code::BadRequest, e))?;
        let pending = lock(&self.pending).remove(&mac);
        if !ok {
            return Ok(());
        }
        let Some(name) = pending else {
            return Err(Refusal::new(Code::BadRequest, "no pairing with that Mac is waiting to be confirmed"));
        };
        let mut approvers = self.approvers();
        approvers.retain(|d| d.fp.parse::<Fingerprint>().ok() != Some(mac));
        approvers.push(Device { fp: mac.to_string(), name, added: crate::now_ms() });
        let json = serde_json::to_vec_pretty(&approvers)
            .map_err(|e| Refusal::new(Code::Failed, format!("could not write the list of Macs: {e}")))?;
        write_private(&self.local.join(APPROVERS), &json)
            .map_err(|e| Refusal::new(Code::Failed, format!("could not write {APPROVERS}: {e}")))
    }

    /// Run `work` on this device's network runtime, made the first time it is needed.
    fn run<T: Send>(&self, work: impl std::future::Future<Output = T> + Send) -> Result<T, Refusal> {
        let runtime = self.runtime.get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .thread_name("spatikey-ask")
                .enable_all()
                .build()
                .map_err(|e| format!("could not start the network runtime: {e}"))
        });
        let runtime = runtime.as_ref().map_err(|e| Refusal::new(Code::Failed, e.clone()))?;
        if tokio::runtime::Handle::try_current().is_ok() {
            // Called from inside some other runtime, which may not block: wait on a thread of
            // its own.
            std::thread::scope(|scope| scope.spawn(|| runtime.block_on(work)).join())
                .map_err(|_| Refusal::new(Code::Failed, "the request's thread failed"))
        } else {
            Ok(runtime.block_on(work))
        }
    }

    async fn ask(&self, paired: &[Fingerprint], framed: &[u8], id: u64) -> Result<Reply, Refusal> {
        let mut fresh = false;
        loop {
            let (link, kept) = self.link(paired, fresh).await?;
            match exchange(&link.connection, framed, ANSWER_WAIT).await {
                Ok(reply) if reply.id != id => {
                    self.forget(&link);
                    return Err(Refusal::new(Code::Failed, "the Mac answered another request"));
                }
                Ok(reply) => {
                    let Some(refusal) = reply.err.as_ref().filter(|r| r.code == Code::NotSameLan) else {
                        return Ok(reply);
                    };
                    // A kept connection may have outlived the Mac's memory of this device's last
                    // broadcast: once more, with a broadcast and a new connection.
                    self.forget(&link);
                    if kept && !fresh {
                        fresh = true;
                        continue;
                    }
                    return Ok(Reply::err(id, Refusal::new(Code::NotSameLan, format!("{} {TUNNEL_HINT}", refusal.message))));
                }
                // Nothing reached the Mac, on a connection kept from before: it had gone.
                Err(Lost::Before(_)) if kept && !fresh => {
                    self.forget(&link);
                    fresh = true;
                }
                Err(lost) => {
                    if !link.alive() {
                        self.forget(&link);
                    }
                    return Err(lost.refusal());
                }
            }
        }
    }

    /// The kept connection, if it is still good -- heard again first if its Mac last answered a
    /// broadcast too long ago -- or else a broadcast and a new connection to a paired Mac that
    /// answers it. Says whether the connection was a kept one.
    async fn link(&self, paired: &[Fingerprint], fresh: bool) -> Result<(Arc<Link>, bool), Refusal> {
        let _one_at_a_time = self.connecting.lock().await;
        let current = lock(&self.link).clone();
        if let Some(link) = current {
            if !fresh && link.alive() && paired.contains(&link.mac) {
                if link.heard_ago() <= STALE {
                    return Ok((link, true));
                }
                let mac = link.mac;
                let (answered, _) = self.beacons.discover(false, FIND_WAIT, |a| a.fp == mac).await;
                if answered.iter().any(|a| a.fp == mac) {
                    link.heard_now();
                    return Ok((link, true));
                }
            }
            self.forget(&link);
        }
        // Before each connection, a broadcast: it finds the Mac, and it is what the Mac will
        // judge the connection by.
        let (answered, failure) = self.beacons.discover(false, FIND_WAIT, |a| paired.contains(&a.fp)).await;
        let Some(mac) = answered.into_iter().find(|a| paired.contains(&a.fp)) else {
            return Err(not_found(failure, "no Mac this device is paired with answered on this network"));
        };
        let link = Arc::new(self.connect(&mac).await?);
        if let Some(old) = lock(&self.link).replace(link.clone()) {
            old.connection.close(0u32.into(), b"replaced");
        }
        self.keep_heard(&link);
        Ok((link, false))
    }

    async fn connect(&self, mac: &Answered) -> Result<Link, Refusal> {
        // From this device's own address on the Mac's network, so that the packets leave by
        // that network: a tunnel's route would bring them to the Mac from somewhere else, to be
        // refused.
        let lans = self.beacons.network.lans();
        let Some(lan) = lan::lan_in(&lans, mac.ip).cloned() else {
            return Err(Refusal::new(
                Code::NotReachable,
                format!("{} answered from {}, which is not on this device's network", mac.name, mac.ip),
            ));
        };
        let socket = UdpSocket::bind(SocketAddr::new(lan.addr, 0))
            .map_err(|e| Refusal::new(Code::Failed, format!("could not open a socket on {}: {e}", lan.addr)))?;
        tie_to_device(&socket, &lan);
        let endpoint = transport::client_on(&self.identity, vec![mac.fp], &[ALPN], socket)
            .map_err(|e| Refusal::new(Code::Failed, e))?;
        let target = SocketAddr::new(mac.ip, mac.port);
        let connecting = endpoint
            .connect(target, SERVER_NAME)
            .map_err(|e| Refusal::new(Code::Failed, format!("could not start a connection to {target}: {e}")))?;
        let connection = match tokio::time::timeout(CONNECT_WAIT, connecting).await {
            Err(_) => return Err(Refusal::new(Code::NotReachable, format!("{} did not answer at {target}", mac.name))),
            Ok(Err(e)) => return Err(refusal_of(&e)),
            Ok(Ok(connection)) => connection,
        };
        if !speaks_spatikey(&connection) {
            connection.close(1u32.into(), b"not spatikey/1");
            return Err(Refusal::new(Code::NotReachable, format!("what answered at {target} is not SpatiKey")));
        }
        Ok(Link { endpoint, connection, mac: mac.fp, heard: Mutex::new(Instant::now()) })
    }

    /// While a connection is kept, a broadcast every [`ASK_EVERY`], so that the Mac keeps
    /// hearing this device; it stops when the connection goes.
    fn keep_heard(&self, link: &Arc<Link>) {
        let kept = Arc::downgrade(link);
        let beacons = self.beacons.clone();
        let mac = link.mac;
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(ASK_EVERY).await;
                match kept.upgrade() {
                    Some(link) if link.alive() => {}
                    _ => return,
                }
                let (answered, _) = beacons.discover(false, FIND_WAIT, |a| a.fp == mac).await;
                match kept.upgrade() {
                    Some(link) if answered.iter().any(|a| a.fp == mac) => link.heard_now(),
                    Some(_) => log::info!("the Mac did not answer this device's broadcast; it may have left the network"),
                    None => return,
                }
            }
        });
    }

    fn forget(&self, link: &Arc<Link>) {
        {
            let mut current = lock(&self.link);
            if current.as_ref().is_some_and(|kept| Arc::ptr_eq(kept, link)) {
                *current = None;
            }
        }
        link.connection.close(0u32.into(), b"done");
    }
}

impl Drop for Asker {
    fn drop(&mut self) {
        let link = self.link.get_mut().ok().and_then(|link| link.take());
        if let Some(Ok(runtime)) = self.runtime.take() {
            if let Some(link) = link {
                link.connection.close(0u32.into(), b"done");
                // A moment for the close to reach the Mac, which ends this device's sessions
                // then, rather than when the connection times out.
                if tokio::runtime::Handle::try_current().is_err() {
                    let endpoint = link.endpoint.clone();
                    runtime.block_on(async move {
                        let _ = tokio::time::timeout(Duration::from_millis(200), endpoint.wait_idle()).await;
                    });
                }
            }
            runtime.shutdown_background();
        }
    }
}

/// A connection kept to a Mac.
struct Link {
    endpoint: quinn::Endpoint,
    connection: quinn::Connection,
    mac: Fingerprint,
    /// When the Mac last answered this device's broadcast.
    heard: Mutex<Instant>,
}

impl Link {
    fn alive(&self) -> bool {
        self.connection.close_reason().is_none()
    }

    fn heard_ago(&self) -> Duration {
        lock(&self.heard).elapsed()
    }

    fn heard_now(&self) {
        *lock(&self.heard) = Instant::now();
    }
}

/// A Mac that answered a broadcast.
#[derive(Debug, Clone)]
struct Answered {
    fp: Fingerprint,
    name: String,
    ip: IpAddr,
    port: u16,
}

/// What a device broadcasts with, apart from the device, for the task that keeps broadcasting
/// while a connection is kept.
#[derive(Clone)]
struct Beacons {
    network: Network,
    port: u16,
    fp: String,
    name: String,
}

impl Beacons {
    /// Broadcast `ASK` (or `PAIR`), and gather the Macs that answer within `wait` -- sooner once
    /// `enough` says one will do. With none, also what stopped the broadcast going, if anything did.
    async fn discover(&self, pair: bool, wait: Duration, enough: impl Fn(&Answered) -> bool) -> (Vec<Answered>, Option<String>) {
        let nonce = nonce();
        let beacon = if pair {
            Beacon::Pair { nonce: nonce.clone(), fp: self.fp.clone(), name: self.name.clone() }
        } else {
            Beacon::Ask { nonce: nonce.clone(), fp: self.fp.clone(), name: self.name.clone() }
        }
        .encode();
        let socket = match tokio::net::UdpSocket::bind(SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), 0)).await {
            Ok(socket) => socket,
            Err(e) => return (Vec::new(), Some(format!("could not open a socket: {e}"))),
        };
        if let Err(e) = socket.set_broadcast(true) {
            return (Vec::new(), Some(format!("could not broadcast: {e}")));
        }
        let destinations = self.network.destinations(self.port);
        let deadline = tokio::time::Instant::now() + wait;
        let mut answered: Vec<Answered> = Vec::new();
        let mut went = false;
        let mut failure = None;
        let mut sends = 0;
        let mut next_send = tokio::time::Instant::now();
        let mut buffer = [0u8; 2048];
        loop {
            let now = tokio::time::Instant::now();
            if now >= next_send && sends < SENDS {
                for to in &destinations {
                    match socket.send_to(beacon.as_bytes(), to).await {
                        Ok(_) => went = true,
                        Err(e) => {
                            log::debug!("could not broadcast to {to}: {e}");
                            failure = Some(format!("{to}: {e}"));
                        }
                    }
                }
                sends += 1;
                next_send = now + RESEND;
            }
            if now >= deadline {
                break;
            }
            let until = if sends < SENDS { next_send.min(deadline) } else { deadline };
            match tokio::time::timeout_at(until, socket.recv_from(&mut buffer)).await {
                Ok(Ok((length, from))) => {
                    if let Some(mac) = self.answer(&buffer[..length], from, &nonce) {
                        if !answered.iter().any(|a| a.fp == mac.fp) {
                            let done = enough(&mac);
                            answered.push(mac);
                            if done {
                                break;
                            }
                        }
                    }
                }
                Ok(Err(e)) => {
                    log::debug!("while listening for Macs: {e}");
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                Err(_) => {}
            }
        }
        let failure = if answered.is_empty() && !went { failure } else { None };
        (answered, failure)
    }

    fn answer(&self, bytes: &[u8], from: SocketAddr, nonce: &str) -> Option<Answered> {
        let Beacon::Here { nonce: said, fp, port, name } = Beacon::decode(std::str::from_utf8(bytes).ok()?)? else {
            return None;
        };
        if said != nonce {
            return None;
        }
        let ip = lan::canonical(from.ip());
        // The device's own look at where the answer came from. It decides nothing -- the Mac
        // does -- but a Mac heard through a tunnel would only refuse.
        if lan::lan_in(&self.network.lans(), ip).is_none() {
            log::debug!("{name} answered from {ip}, which is not on this device's network; not asked");
            return None;
        }
        Some(Answered { fp: fp.parse().ok()?, name, ip, port })
    }
}

// MARK: both sides

/// What became of a request that got no answer.
enum Lost {
    /// It never reached the Mac: safe to send again, on a new connection.
    Before(Refusal),
    /// It went, and no answer came back. Not sent again, since a person may already be asked.
    After(Refusal),
}

impl Lost {
    fn refusal(self) -> Refusal {
        match self {
            Lost::Before(refusal) | Lost::After(refusal) => refusal,
        }
    }
}

/// One request on its own stream, and its answer.
async fn exchange(connection: &quinn::Connection, framed: &[u8], wait: Duration) -> Result<Reply, Lost> {
    let (mut send, mut recv) = connection.open_bi().await.map_err(|e| Lost::Before(refusal_of(&e)))?;
    if let Err(e) = send.write_all(framed).await {
        return Err(Lost::Before(match e {
            quinn::WriteError::ConnectionLost(e) => refusal_of(&e),
            other => Refusal::new(Code::NotReachable, format!("the request could not be sent: {other}")),
        }));
    }
    let _ = send.finish();
    let body = match tokio::time::timeout(wait, read_message(&mut recv)).await {
        Err(_) => return Err(Lost::After(Refusal::new(Code::Timeout, "the Mac did not answer in two minutes"))),
        Ok(Err(Unread::Lost(e))) => {
            let refusal = refusal_of(&e);
            // A Mac that restarted has no memory of the connection, and resets it: it was asked
            // nothing.
            return Err(match e {
                quinn::ConnectionError::Reset => Lost::Before(refusal),
                _ => Lost::After(refusal),
            });
        }
        Ok(Err(other)) => return Err(Lost::After(Refusal::new(Code::Failed, format!("the Mac's answer did not arrive: {other}")))),
        Ok(Ok(body)) => body,
    };
    serde_json::from_slice(&body).map_err(|e| {
        Lost::After(Refusal::new(
            Code::Failed,
            format!("the Mac's answer could not be read (line {}, column {})", e.line(), e.column()),
        ))
    })
}

/// Why a message could not be read.
enum Unread {
    Lost(quinn::ConnectionError),
    Other(String),
}

impl std::fmt::Display for Unread {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Unread::Lost(e) => write!(f, "{e}"),
            Unread::Other(e) => write!(f, "{e}"),
        }
    }
}

/// One message in [`wire`]'s framing, from a stream.
async fn read_message(recv: &mut quinn::RecvStream) -> Result<Zeroizing<Vec<u8>>, Unread> {
    fn unread(e: quinn::ReadExactError) -> Unread {
        match e {
            quinn::ReadExactError::ReadError(quinn::ReadError::ConnectionLost(e)) => Unread::Lost(e),
            other => Unread::Other(other.to_string()),
        }
    }
    let mut length = [0u8; 4];
    recv.read_exact(&mut length).await.map_err(unread)?;
    let length = u32::from_be_bytes(length) as usize;
    if length > wire::MAX_MESSAGE {
        return Err(Unread::Other(format!("a message of {length} bytes, larger than any request")));
    }
    let mut body = Zeroizing::new(vec![0u8; length]);
    recv.read_exact(&mut body[..]).await.map_err(unread)?;
    Ok(body)
}

fn frame(env: &Envelope) -> Result<Zeroizing<Vec<u8>>, Refusal> {
    let json = Zeroizing::new(
        serde_json::to_vec(env).map_err(|e| Refusal::new(Code::Failed, format!("could not write the request: {e}")))?,
    );
    let mut framed = Zeroizing::new(Vec::with_capacity(json.len() + 4));
    wire::write_frame(&mut *framed, &json).map_err(|e| Refusal::new(Code::BadRequest, format!("{e}")))?;
    Ok(framed)
}

/// What a lost connection means to the device.
fn refusal_of(e: &quinn::ConnectionError) -> Refusal {
    // TLS alerts travel as QUIC's crypto errors, 0x100 to 0x1ff.
    fn tls(code: quinn::TransportErrorCode) -> bool {
        u64::from(code) & !0xff == 0x100
    }
    match e {
        // The Mac turned this device's certificate down in the handshake. In TLS 1.3 that is
        // seen only after the connection seemed made, as its closing.
        quinn::ConnectionError::ConnectionClosed(close) if tls(close.error_code) => Refusal::new(
            Code::NoApprover,
            "the Mac does not let this device in: it is not paired with it, or no longer. Pair them again",
        ),
        // This end turned the Mac's down.
        quinn::ConnectionError::TransportError(error) if tls(error.code) => {
            Refusal::new(Code::NotReachable, "what answered is not the Mac this device paired with")
        }
        quinn::ConnectionError::TimedOut => Refusal::new(Code::NotReachable, "the Mac stopped answering"),
        quinn::ConnectionError::ApplicationClosed(close) => Refusal::new(
            Code::NotReachable,
            format!("the Mac closed the connection ({})", String::from_utf8_lossy(&close.reason)),
        ),
        other => Refusal::new(Code::NotReachable, format!("the connection to the Mac failed: {other}")),
    }
}

/// Whether the handshake agreed on `spatikey/1`. The handshake refuses a disagreement already;
/// this is the second look, which costs nothing.
fn speaks_spatikey(connection: &quinn::Connection) -> bool {
    connection
        .handshake_data()
        .and_then(|data| data.downcast::<quinn::crypto::rustls::HandshakeData>().ok())
        .is_some_and(|data| data.protocol.as_deref() == Some(ALPN))
}

/// On Linux, the socket is tied to the LAN's interface as well, which outranks any route a
/// tunnel has put in: unprivileged since Linux 5.7, and refused (EPERM) to an Android app,
/// where binding to the address is all there is.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn tie_to_device(socket: &UdpSocket, lan: &Lan) {
    use std::os::fd::AsRawFd;
    let Ok(name) = std::ffi::CString::new(lan.name.as_str()) else {
        return;
    };
    // SAFETY: a socket this process owns, and a NUL-terminated name the call only reads.
    let result = unsafe {
        libc::setsockopt(
            socket.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_BINDTODEVICE,
            name.as_ptr() as *const libc::c_void,
            name.as_bytes_with_nul().len() as libc::socklen_t,
        )
    };
    if result != 0 {
        let e = std::io::Error::last_os_error();
        if e.raw_os_error() == Some(libc::EPERM) {
            log::debug!("not allowed to tie the socket to {}; it is bound to {} only", lan.name, lan.addr);
        } else {
            log::info!("could not tie the socket to {}: {e}", lan.name);
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn tie_to_device(_socket: &UdpSocket, _lan: &Lan) {}

/// A name a device gave, fit to show in a prompt: no control characters, none that turn the
/// text's direction around, at most 64 characters, trimmed.
fn clean_name(name: &str) -> String {
    let kept: String = name
        .chars()
        .filter(|&c| !c.is_control() && !matches!(c, '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'))
        .take(64)
        .collect();
    kept.trim().to_string()
}

/// The `id` of a request that would not read, if that much of it can be read.
fn id_of(bytes: &[u8]) -> u64 {
    serde_json::from_slice::<serde_json::Value>(bytes).ok().and_then(|value| value.get("id")?.as_u64()).unwrap_or(0)
}

fn same_fp(a: &str, b: &str) -> bool {
    matches!((a.parse::<Fingerprint>(), b.parse::<Fingerprint>()), (Ok(a), Ok(b)) if a == b)
}

fn not_found(failure: Option<String>, otherwise: &str) -> Refusal {
    Refusal::new(
        Code::NotReachable,
        match failure {
            Some(failure) => format!("could not broadcast on this network ({failure})"),
            None => otherwise.to_string(),
        },
    )
}

/// 16 random hex characters: what ties a Mac's answer to the broadcast it answers.
fn nonce() -> String {
    use ring::rand::SecureRandom;
    let mut bytes = [0u8; 8];
    if ring::rand::SystemRandom::new().fill(&mut bytes).is_err() {
        // It matches an answer, and is never trusted for more; the clock will do.
        bytes = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
            .to_be_bytes();
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Write a file of this machine's own whole or not at all: a hidden file beside it, flushed to
/// disk, renamed over it; readable by this user only.
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let folder = path
        .parent()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "a file with no folder"))?;
    std::fs::create_dir_all(folder)?;
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let temporary = folder.join(format!(".{name}.{}.new", std::process::id()));
    let mut file = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&temporary, path)
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_shown_in_a_prompt_cannot_turn_the_text_around_or_run_on() {
        assert_eq!(clean_name("  Steam Deck\n"), "Steam Deck");
        assert_eq!(clean_name("evil\u{202e}kced"), "evilkced");
        assert_eq!(clean_name(&"x".repeat(100)).len(), 64);
    }

    #[test]
    fn a_request_that_does_not_read_is_answered_by_its_id_when_it_has_one() {
        assert_eq!(id_of(br#"{"id":9,"op":"nonsense"}"#), 9);
        assert_eq!(id_of(b"not json"), 0);
    }
}
