//! Remote applications on the Beam Pro: the same hosts, the same protocol, the same pairing as
//! the Deck's (`crates/spatiand/src/remote`), with Android's decoder and without a compositor.
//!
//! On the Deck a remote window becomes a Wayland client's surface, and the compositor does the
//! rest. Here there is no compositor to hand it to, so this keeps the list of windows itself
//! ([`View`]) and the drawing thread reads it. What it keeps is what the Deck's session keeps
//! -- which window is which application, how its pictures are packed, whether it is a window
//! or the room -- and it follows the same two rules about frames: decode every one, and never
//! feed the decoder a frame with a hole in it.
//!
//! The head goes back to the host exactly as the Deck sends it: a `Viewport` per drawn frame,
//! as a datagram, built from the same eyes the room is drawn with.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use spatiand_stream::link::{hear, resolve};
use spatiand_stream::video::{Arrival, Packet, Reassembler};
use spatiand_stream::{ClientMessage, Codec, Eyes, Fingerprint, HostMessage, Identity, Layer, WindowId};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use super::decode::{self, Decoder, Output};

/// What the session says it is, to a host's log.
const SESSION: &str = "spatiand on the Beam Pro";

/// A host this phone has paired with. `hosts.toml` in the config directory, one `[[host]]` each.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostEntry {
    /// Name or address, with a port: `myhost:47600`.
    pub address: String,
    /// The host's certificate, as pairing learned it.
    pub fingerprint: String,
    /// Applications to ask for as soon as the link is up.
    #[serde(default)]
    pub launch: Vec<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct HostsFile {
    #[serde(default)]
    host: Vec<HostEntry>,
}

fn hosts_path() -> PathBuf {
    spatiand_track::config::config_dir().join("hosts.toml")
}

pub fn load_hosts() -> Vec<HostEntry> {
    std::fs::read_to_string(hosts_path())
        .ok()
        .and_then(|text| toml::from_str::<HostsFile>(&text).ok())
        .map(|file| file.host)
        .unwrap_or_default()
}

fn save_host(entry: HostEntry) -> Result<(), String> {
    let mut hosts = load_hosts();
    // Pairing again with a host already known replaces it.
    hosts.retain(|h| h.address != entry.address);
    hosts.insert(0, entry);
    let text = toml::to_string(&HostsFile { host: hosts }).map_err(|e| e.to_string())?;
    let path = hosts_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    std::fs::write(&path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// One remote window, as the drawing thread needs to know it.
#[derive(Clone)]
pub struct Window {
    pub app: String,
    pub title: String,
    /// How its pictures are packed, and whether it is a window or the room.
    pub eyes: Eyes,
    pub layer: Layer,
    /// Where its pictures come out, once the host has said what it streams.
    pub output: Option<Arc<Output>>,
}

/// What the session knows, for the drawing thread and the phone's screen.
#[derive(Default)]
pub struct View {
    /// One line: connecting, online, or why not.
    pub link: String,
    pub host_name: Option<String>,
    /// What the host offers: (id, name).
    pub apps: Vec<(String, String)>,
    pub windows: BTreeMap<u32, Window>,
}

pub enum Command {
    Launch(String),
    Close(u32),
    Viewport(spatiand_stream::Viewport),
}

/// What the rest of the app holds of a host: its view, and a way to ask it things.
#[derive(Clone)]
pub struct Handle {
    pub view: Arc<Mutex<View>>,
    commands: UnboundedSender<Command>,
}

impl Handle {
    pub fn launch(&self, app: &str) {
        let _ = self.commands.send(Command::Launch(app.to_string()));
    }

    pub fn close(&self, window: u32) {
        let _ = self.commands.send(Command::Close(window));
    }

    pub fn viewport(&self, viewport: spatiand_stream::Viewport) {
        let _ = self.commands.send(Command::Viewport(viewport));
    }
}

/// A host being shown: its thread, which stops when this is dropped.
pub struct Remote {
    pub handle: Handle,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for Remote {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Remote {
    pub fn start(entry: HostEntry) -> Result<Remote, String> {
        let fingerprint: Fingerprint = entry.fingerprint.parse()?;
        let view: Arc<Mutex<View>> = Arc::new(Mutex::new(View {
            link: format!("connecting to {}", entry.address),
            ..View::default()
        }));
        let (commands, inbox) = tokio::sync::mpsc::unbounded_channel();
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let view = view.clone();
            let stop = stop.clone();
            std::thread::Builder::new()
                .name("remote".into())
                .spawn(move || {
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        run(entry, fingerprint, &stop, &view, inbox)
                    }));
                    if result.is_err() {
                        log::error!("the remote session died");
                        view.lock().unwrap().link = "the remote session died; see the log".into();
                    }
                })
                .map_err(|e| format!("could not start the remote thread: {e}"))?
        };
        Ok(Remote {
            handle: Handle { view, commands },
            stop,
            thread: Some(thread),
        })
    }
}

enum Ended {
    Stopped,
    Refused(String),
    Lost(String),
}

fn set_link(view: &Mutex<View>, line: String) {
    view.lock().unwrap().link = line;
}

fn run(
    entry: HostEntry,
    fingerprint: Fingerprint,
    stop: &AtomicBool,
    view: &Mutex<View>,
    mut commands: UnboundedReceiver<Command>,
) {
    let identity = match Identity::load_or_create(&spatiand_track::config::config_dir()) {
        Ok(identity) => identity,
        Err(e) => return set_link(view, format!("no identity: {e}")),
    };
    log::info!(
        "remote: this phone is {}, looking for {}",
        identity.fingerprint().short(),
        entry.address
    );
    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(e) => return set_link(view, format!("no network runtime: {e}")),
    };
    runtime.block_on(async {
        let mut pause = Duration::from_secs(1);
        let mut last_said = String::new();
        while !stop.load(Ordering::Relaxed) {
            set_link(view, format!("connecting to {}", entry.address));
            let ended = match connect(&identity, &entry.address, fingerprint).await {
                Ok(connection) => {
                    log::info!("remote: connected to {}", entry.address);
                    pause = Duration::from_secs(1);
                    last_said.clear();
                    let ended = serve(&connection, &entry, stop, view, &mut commands).await;
                    connection.close(0u32.into(), b"session ended");
                    ended
                }
                Err(e) => Ended::Lost(e),
            };
            {
                let mut v = view.lock().unwrap();
                v.windows.clear();
                v.host_name = None;
            }
            let reason = match ended {
                Ended::Stopped => break,
                Ended::Refused(reason) => {
                    set_link(view, format!("{} refused: {reason}", entry.address));
                    reason
                }
                Ended::Lost(reason) => {
                    set_link(view, format!("{} offline: {reason}", entry.address));
                    reason
                }
            };
            if reason != last_said {
                log::info!("remote {}: {reason}; will keep trying", entry.address);
                last_said = reason;
            }
            let until = Instant::now() + pause;
            while Instant::now() < until && !stop.load(Ordering::Relaxed) {
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_millis(100)) => {}
                    command = commands.recv() => {
                        if let Some(Command::Launch(app)) = command {
                            log::info!("remote: cannot start {app}: {} is not connected", entry.address);
                        }
                    }
                }
            }
            pause = (pause * 2).min(Duration::from_secs(20));
        }
    });
}

async fn connect(identity: &Identity, address: &str, fingerprint: Fingerprint) -> Result<quinn::Connection, String> {
    let endpoint = spatiand_stream::transport::client(identity, fingerprint)?;
    let target = resolve(address).await?;
    let connecting = endpoint
        .connect(target, "spatiand")
        .map_err(|e| format!("cannot reach {target}: {e}"))?;
    tokio::time::timeout(Duration::from_secs(5), connecting)
        .await
        .map_err(|_| format!("{address} does not answer"))?
        .map_err(|e| format!("{target} would not have this phone: {e}"))
}

/// One window's decoding.
struct Stream {
    decoder: Decoder,
    /// Something was lost, so nothing can be decoded correctly until a keyframe arrives.
    broken: bool,
    /// The number the next frame should carry; any other means a frame went missing whole.
    expected: Option<u32>,
    asked: Option<Instant>,
    decoded: u64,
    skipped: u64,
}

fn window_title(view: &View, app: &str, title: &str) -> String {
    let name = view.apps.iter().find(|(id, _)| id == app).map(|(_, name)| name.clone());
    let what = if title.trim().is_empty() {
        name.unwrap_or_else(|| app.to_string())
    } else {
        title.trim().to_string()
    };
    match &view.host_name {
        Some(host) => format!("{what} — {host}"),
        None => what,
    }
}

async fn serve(
    connection: &quinn::Connection,
    entry: &HostEntry,
    stop: &AtomicBool,
    view: &Mutex<View>,
    commands: &mut UnboundedReceiver<Command>,
) -> Ended {
    let mut streams: HashMap<u32, Stream> = HashMap::new();
    let mut reassemblers: HashMap<u32, Reassembler> = HashMap::new();
    let (out, outbox) = tokio::sync::mpsc::unbounded_channel::<ClientMessage>();
    let writer = tokio::spawn(talk(connection.clone(), outbox));
    let say = |message: ClientMessage| {
        let _ = out.send(message);
    };

    let codecs: Vec<Codec> = [Codec::H265, Codec::H264]
        .into_iter()
        .filter(|c| decode::can_decode(*c))
        .collect();
    log::info!("remote: this phone decodes {:?}", codecs);
    say(ClientMessage::Hello {
        version: spatiand_stream::VERSION,
        codecs,
        max_size: (3840, 2160),
        // The Beam Pro draws every display at the phone's 60 Hz.
        refresh_mhz: 60_000,
        session: SESSION.into(),
    });
    for app in &entry.launch {
        say(ClientMessage::Launch { app: app.clone() });
    }

    let control = match connection.accept_uni().await {
        Ok(stream) => stream,
        Err(e) => {
            return Ended::Refused(format!("the host would not talk to this phone ({e}); pair it again"))
        }
    };
    let mut control = Box::pin(hear(control));
    let mut said = Instant::now();
    let mut frames = 0u64;

    while !stop.load(Ordering::Relaxed) {
        tokio::select! {
            message = &mut control => {
                let Some((message, rest)) = message else {
                    return Ended::Lost(match connection.close_reason() {
                        Some(quinn::ConnectionError::TimedOut) => "the link went quiet".into(),
                        Some(reason) => format!("the host closed the link: {reason}"),
                        None => "the host closed the control stream".into(),
                    });
                };
                control = Box::pin(hear(rest));
                match message {
                    HostMessage::Welcome { version, host, reattached } => {
                        if version != spatiand_stream::VERSION {
                            return Ended::Refused(format!(
                                "{host} speaks version {version} and this app {}; update the one that is behind",
                                spatiand_stream::VERSION
                            ));
                        }
                        log::info!("remote: {host} says hello{}", if reattached { ", with windows already open" } else { "" });
                        let mut v = view.lock().unwrap();
                        v.link = format!("online: {host}");
                        v.host_name = Some(host);
                    }
                    HostMessage::Catalog { apps } => {
                        log::info!("remote: {} application(s) offered", apps.len());
                        view.lock().unwrap().apps =
                            apps.into_iter().map(|a| (a.id, a.name)).collect();
                    }
                    HostMessage::Opened(info) => {
                        log::info!("remote: window {} is {} ({}x{})", info.window.0, info.app, info.width, info.height);
                        let mut v = view.lock().unwrap();
                        let title = window_title(&v, &info.app, &info.title);
                        v.windows.insert(info.window.0, Window {
                            app: info.app,
                            title,
                            eyes: Eyes::Mono,
                            layer: Layer::Window,
                            output: None,
                        });
                    }
                    HostMessage::Retitled { window, title } => {
                        let mut v = view.lock().unwrap();
                        if let Some(app) = v.windows.get(&window.0).map(|w| w.app.clone()) {
                            let title = window_title(&v, &app, &title);
                            if let Some(w) = v.windows.get_mut(&window.0) {
                                w.title = title;
                            }
                        }
                    }
                    HostMessage::Stream { window, codec, width, height, eyes } => {
                        match Decoder::new(codec, width, height) {
                            Ok(decoder) => {
                                log::info!("remote: window {} streams {} at {width}x{height}, {eyes:?}", window.0, codec.label());
                                if let Some(w) = view.lock().unwrap().windows.get_mut(&window.0) {
                                    w.eyes = eyes;
                                    w.output = Some(decoder.output());
                                }
                                streams.insert(window.0, Stream {
                                    decoder,
                                    broken: true,
                                    expected: None,
                                    asked: None,
                                    decoded: 0,
                                    skipped: 0,
                                });
                                reassemblers.insert(window.0, Reassembler::new());
                            }
                            Err(e) => log::error!("remote: window {}: {e}", window.0),
                        }
                    }
                    HostMessage::Layer { window, layer } => {
                        log::info!("remote: window {} is now {layer:?}", window.0);
                        if let Some(w) = view.lock().unwrap().windows.get_mut(&window.0) {
                            w.layer = layer;
                        }
                    }
                    HostMessage::Closed { window } => {
                        view.lock().unwrap().windows.remove(&window.0);
                        streams.remove(&window.0);
                        reassemblers.remove(&window.0);
                    }
                    HostMessage::Refused { reason, .. } => return Ended::Refused(reason),
                    other => log::debug!("remote: {other:?}"),
                }
            }
            // Each application's sound comes on a stream of its own. Nothing plays it here
            // yet, but it is read to the end so the host is never held up sending it.
            incoming = connection.accept_uni() => {
                match incoming {
                    Ok(mut stream) => {
                        tokio::spawn(async move {
                            let mut sink = vec![0u8; 16 * 1024];
                            while let Ok(Some(_)) = stream.read(&mut sink).await {}
                        });
                    }
                    Err(_) => return Ended::Lost("the link went quiet".into()),
                }
            }
            datagram = connection.read_datagram() => {
                let Ok(datagram) = datagram else {
                    return Ended::Lost("the link went quiet".into());
                };
                let Some(packet) = Packet::read(&datagram) else { continue };
                let window = packet.window as u32;
                let Some(reassembler) = reassemblers.get_mut(&window) else { continue };
                let Some(stream) = streams.get_mut(&window) else { continue };
                match reassembler.accept(&packet) {
                    Arrival::Frame(frame) => {
                        let gap = stream.expected.is_some_and(|e| e != frame.frame);
                        stream.expected = Some(frame.frame.wrapping_add(1));
                        if gap && !frame.keyframe {
                            stream.broken = true;
                        }
                        if frame.keyframe {
                            stream.broken = false;
                        }
                        if stream.broken {
                            stream.skipped += 1;
                            if stream.asked.is_none_or(|at| at.elapsed() >= Duration::from_millis(500)) {
                                stream.asked = Some(Instant::now());
                                say(ClientMessage::WantKeyframe { window: WindowId(window) });
                            }
                            continue;
                        }
                        match stream.decoder.decode(frame.captured_us, frame.viewport, &frame.bytes, frame.keyframe) {
                            Ok(()) => {
                                stream.decoded += 1;
                                frames += 1;
                            }
                            Err(e) => {
                                log::warn!("remote: window {window}: {e}");
                                stream.broken = true;
                                stream.asked = Some(Instant::now());
                                say(ClientMessage::WantKeyframe { window: WindowId(window) });
                            }
                        }
                    }
                    Arrival::Lost(_) => {
                        stream.broken = true;
                        stream.asked = Some(Instant::now());
                        say(ClientMessage::WantKeyframe { window: WindowId(window) });
                    }
                    Arrival::Partial | Arrival::Stale => {}
                }
            }
            command = commands.recv() => {
                match command {
                    Some(Command::Launch(app)) => {
                        log::info!("remote: asking {} to start {app}", entry.address);
                        say(ClientMessage::Launch { app });
                    }
                    Some(Command::Close(window)) => {
                        say(ClientMessage::Close { window: WindowId(window) });
                    }
                    Some(Command::Viewport(viewport)) => {
                        // A datagram, never the ordered stream: a head pose is only worth
                        // anything while it is the newest one.
                        if let Ok(bytes) = spatiand_stream::to_bytes(&ClientMessage::Viewport(viewport)) {
                            let _ = connection.send_datagram(bytes.into());
                        }
                    }
                    None => {}
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(5)) => {}
        }

        // A stream still waiting for a keyframe keeps asking: the one it was sent can overtake
        // the message saying the stream exists, and then nothing else comes.
        for (id, stream) in streams.iter_mut() {
            stream.decoder.drain();
            if stream.broken && stream.asked.is_none_or(|at| at.elapsed() >= Duration::from_millis(500)) {
                stream.asked = Some(Instant::now());
                say(ClientMessage::WantKeyframe { window: WindowId(*id) });
            }
        }

        if said.elapsed() >= Duration::from_secs(5) {
            let secs = said.elapsed().as_secs_f32();
            let skipped: u64 = streams.values().map(|s| s.skipped).sum();
            log::info!(
                "remote {}: {frames} decoded in {secs:.1}s, {skipped} skipped for a keyframe, rtt {:.0} ms",
                entry.address,
                connection.rtt().as_secs_f32() * 1000.0
            );
            frames = 0;
            said = Instant::now();
        }
    }
    say(ClientMessage::Detach);
    // Dropping the sender ends the writer, which finishes the stream once the queue is empty.
    drop(out);
    let _ = tokio::time::timeout(Duration::from_millis(200), writer).await;
    Ended::Stopped
}

/// Everything this end says, down one ordered stream. See the Deck's `talk` for why one.
async fn talk(connection: quinn::Connection, mut outbox: UnboundedReceiver<ClientMessage>) {
    let Ok(mut stream) = connection.open_uni().await else { return };
    if stream.write_all(&spatiand_stream::CONTROL_MAGIC).await.is_err() {
        return;
    }
    while let Some(message) = outbox.recv().await {
        let Ok(bytes) = spatiand_stream::to_bytes(&message) else { continue };
        let length = (bytes.len() as u32).to_le_bytes();
        if stream.write_all(&length).await.is_err() || stream.write_all(&bytes).await.is_err() {
            return;
        }
    }
    let _ = stream.finish();
}

// --- pairing ---

/// A pairing in progress: the code both ends show, and how it ended.
#[derive(Default)]
pub struct PairingState {
    pub address: String,
    pub code: Option<String>,
    pub fingerprint: Option<Fingerprint>,
    /// Set once the host has said yes (with what it calls itself) or it failed.
    pub outcome: Option<Result<String, String>>,
    /// The person holding the phone said the codes match.
    pub confirmed: bool,
    /// Written to `hosts.toml`.
    pub saved: bool,
}

pub struct Pairing {
    pub state: Arc<Mutex<PairingState>>,
    _cancel: tokio::sync::oneshot::Sender<()>,
}

impl Pairing {
    pub fn start(address: String) -> Pairing {
        let state = Arc::new(Mutex::new(PairingState {
            address: address.clone(),
            ..PairingState::default()
        }));
        let (cancel, cancelled) = tokio::sync::oneshot::channel::<()>();
        {
            let state = state.clone();
            let _ = std::thread::Builder::new().name("pair".into()).spawn(move || {
                let finish = |outcome: Result<String, String>| state.lock().unwrap().outcome = Some(outcome);
                let identity = match Identity::load_or_create(&spatiand_track::config::config_dir()) {
                    Ok(identity) => identity,
                    Err(e) => return finish(Err(e)),
                };
                let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                    Ok(runtime) => runtime,
                    Err(e) => return finish(Err(format!("no network runtime: {e}"))),
                };
                let outcome = runtime.block_on(async {
                    let progress = |p: spatiand_stream::link::Pairing| {
                        let spatiand_stream::link::Pairing::Compare { fingerprint, code } = p;
                        log::info!("pairing with {address}: code {code}");
                        let mut s = state.lock().unwrap();
                        s.code = Some(code);
                        s.fingerprint = Some(fingerprint);
                    };
                    tokio::select! {
                        outcome = spatiand_stream::link::pair(&identity, &address, progress) => outcome,
                        _ = cancelled => Err("cancelled".into()),
                    }
                });
                match &outcome {
                    Ok(paired) => {
                        log::info!("{address} said yes; it is {}", paired.name);
                        let mut s = state.lock().unwrap();
                        s.fingerprint = Some(paired.fingerprint);
                    }
                    Err(e) => log::info!("pairing with {address} ended: {e}"),
                }
                finish(outcome.map(|p| p.name));
            });
        }
        Pairing { state, _cancel: cancel }
    }

    /// Once both the host and the person have said yes, write the host down. Returns the
    /// entry the first time that happens.
    pub fn settle(&self) -> Option<HostEntry> {
        let mut s = self.state.lock().unwrap();
        if s.saved || !s.confirmed || !matches!(s.outcome, Some(Ok(_))) {
            return None;
        }
        let entry = HostEntry {
            address: s.address.clone(),
            fingerprint: s.fingerprint?.to_string(),
            launch: Vec::new(),
        };
        match save_host(entry.clone()) {
            Ok(()) => {
                s.saved = true;
                log::info!("paired with {}", s.address);
                Some(entry)
            }
            Err(e) => {
                s.outcome = Some(Err(e));
                None
            }
        }
    }

    /// One line for the phone's screen.
    pub fn describe(&self) -> String {
        let s = self.state.lock().unwrap();
        match (&s.outcome, &s.code, s.confirmed) {
            (Some(Err(e)), _, _) => format!("pairing with {} failed: {e}", s.address),
            (Some(Ok(name)), _, true) if s.saved => format!("paired with {name}"),
            (Some(Ok(name)), _, false) => format!("{name} said yes; tap \"Codes match\" if they do"),
            (_, None, _) => format!("looking for {}…", s.address),
            (_, Some(code), false) => format!("code {code}: check {} shows the same, answer yes there, then tap \"Codes match\"", s.address),
            (_, Some(code), true) => format!("code {code}: waiting for {} to say yes…", s.address),
        }
    }
}
