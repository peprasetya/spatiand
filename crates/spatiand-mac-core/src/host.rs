//! The Mac as a host: the other end of the link, behind a small C interface.
//!
//! The same protocol the Linux `spatiand-host` speaks, so a Deck or a Beam Pro can show a Mac's
//! windows as it shows any host's. What is different is everything else: there is no compositor
//! here, the windows are the Mac's own, and the pictures come from ScreenCaptureKit and a
//! hardware encoder. So this is only the **transport** -- the listening, the admission of a
//! device, the control stream and the packetising of pictures -- and the Swift side decides
//! what the windows are and what to do about what a session says. A message each way crosses as
//! JSON, as in [`crate`]'s client half; pictures cross as bytes.
//!
//! One session at a time and the newest wins, as on the Linux host (see its `net.rs`): the old
//! connection is closed with `CLOSE_TAKEN_OVER`, and nothing here belongs to a device.

use std::collections::HashMap;
use std::ffi::{c_char, c_void, CStr, CString};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use serde_json::{json, Value};
use spatiand_stream::transport::{peer_fingerprint, Trust, CLOSE_TAKEN_OVER};
use spatiand_stream::video::{split, Packet, FLAG_KEYFRAME, FLAG_LAST};
use spatiand_stream::pairing::{Verdict, Window};
use spatiand_stream::{ClientMessage, Fingerprint, Gate, HostMessage, Identity};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

type EventFn = extern "C" fn(*mut c_void, *const c_char);

/// How long a device not paired yet has to prove the code, once it has connected.
const PROVING: Duration = Duration::from_secs(30);

#[derive(Clone, Copy)]
struct Callbacks {
    user: usize,
    event: EventFn,
}

impl Callbacks {
    fn say(&self, value: &Value) {
        if let Ok(text) = CString::new(value.to_string()) {
            (self.event)(self.user as *mut c_void, text.as_ptr());
        }
    }
}

/// What the app sends out.
enum ToSession {
    Control(HostMessage),
    Video { window: u16, frame: u32, keyframe: bool, captured_us: u64, bytes: Vec<u8> },
    /// A little of one application's sound: signed 16-bit little-endian, interleaved.
    Audio { app: String, channels: u16, pcm: Vec<u8> },
}

pub struct HostCore {
    runtime: tokio::runtime::Runtime,
    out: UnboundedSender<ToSession>,
    gate: Arc<Gate>,
    identity: Arc<Identity>,
    paired: Arc<Mutex<Vec<Fingerprint>>>,
    paired_path: PathBuf,
    /// The pairing window, while one is open: its code, its clock and its attempts. The same
    /// as the Linux host's; see `spatiand_stream::pairing`.
    window: Arc<Mutex<Option<Window>>>,
    /// The next frame number of each window, kept across a restarted encoder: a session drops
    /// any frame numbered at or below the last it showed.
    frames: Mutex<HashMap<u16, u32>>,
}

// MARK: JSON, with the bytes as base64

/// What the app writes, as serde reads it: the byte fields arrive as base64 and go on as arrays.
fn from_app_json(text: &str) -> Result<HostMessage, String> {
    let mut value: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    fn bytes(value: &mut Value) {
        match value {
            Value::Object(map) => {
                for (key, inner) in map.iter_mut() {
                    if key == "icon_png" || key == "bytes" || key == "pixels" {
                        if let Value::String(encoded) = inner {
                            if let Ok(raw) = base64::engine::general_purpose::STANDARD.decode(encoded.as_bytes()) {
                                *inner = Value::Array(raw.into_iter().map(Value::from).collect());
                            }
                        }
                    } else {
                        bytes(inner);
                    }
                }
            }
            Value::Array(items) => items.iter_mut().for_each(bytes),
            _ => {}
        }
    }
    bytes(&mut value);
    serde_json::from_value(value).map_err(|e| e.to_string())
}

/// What a session said, as JSON for the app, with the byte fields as base64.
fn to_app_json(message: &ClientMessage) -> Value {
    let mut value = serde_json::to_value(message).unwrap_or(Value::Null);
    fn bytes(value: &mut Value) {
        match value {
            Value::Object(map) => {
                for (key, inner) in map.iter_mut() {
                    if key == "bytes" || key == "pixels" || key == "icon_png" {
                        if let Value::Array(items) = inner {
                            let raw: Vec<u8> = items.iter().filter_map(|b| b.as_u64().map(|b| b as u8)).collect();
                            *inner = Value::String(base64::engine::general_purpose::STANDARD.encode(raw));
                        }
                    } else {
                        bytes(inner);
                    }
                }
            }
            Value::Array(items) => items.iter_mut().for_each(bytes),
            _ => {}
        }
    }
    bytes(&mut value);
    json!({ "said": value })
}

// MARK: the paired list

fn load_paired(path: &std::path::Path) -> Vec<Fingerprint> {
    std::fs::read_to_string(path)
        .map(|text| text.lines().filter_map(|l| l.split_whitespace().next()?.parse().ok()).collect())
        .unwrap_or_default()
}

fn save_paired(path: &std::path::Path, list: &[Fingerprint]) {
    let text: String = list.iter().map(|f| format!("{f}\n")).collect();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = std::fs::write(path, text) {
        log::warn!("could not write the paired list {}: {e}", path.display());
    }
}

impl HostCore {
    fn start(identity_dir: PathBuf, port: u16, callbacks: Callbacks) -> Result<HostCore, String> {
        let identity = Arc::new(Identity::load_or_create(&identity_dir)?);
        let paired_path = identity_dir.with_file_name("host-paired.txt");
        let paired = Arc::new(Mutex::new(load_paired(&paired_path)));
        let gate = Arc::new(Gate::new(paired.lock().unwrap().clone()));
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(|e| format!("could not start the network runtime: {e}"))?;
        let bind: SocketAddr = format!("[::]:{port}").parse().map_err(|e| format!("{e}"))?;
        let endpoint = {
            let _guard = runtime.enter();
            spatiand_stream::transport::server(bind, &identity, Trust::Gate(gate.clone()))
                .or_else(|_| spatiand_stream::transport::server(format!("0.0.0.0:{port}").parse().unwrap(), &identity, Trust::Gate(gate.clone())))?
        };
        // The owner's devices: one paired with any of their machines may connect here; one
        // removed on any may not, and is hung up on (`serve`, `gone`).
        {
            let (paired, paired_path, gate) = (paired.clone(), paired_path.clone(), gate.clone());
            let me = identity.fingerprint().to_string();
            let known_path = paired_path.clone();
            let list = spatiand_sync::roster::Roster::watch(move || {
                load_paired(&known_path).iter().map(|f| (f.to_string(), String::new())).collect()
            });
            std::thread::spawn(move || {
                for entries in list {
                    let i_am_gone = entries.iter().any(|e| e.fp == me && e.removed());
                    let mut now = paired.lock().unwrap().clone();
                    let before = now.len();
                    for e in &entries {
                        let Ok(fp) = e.fp.parse::<Fingerprint>() else { continue };
                        if e.removed() || i_am_gone {
                            now.retain(|f| *f != fp);
                            crate::key::forget_device(&e.fp);
                        } else if e.fp != me && !now.contains(&fp) {
                            now.push(fp);
                        }
                    }
                    if now.len() != before || *paired.lock().unwrap() != now {
                        *paired.lock().unwrap() = now.clone();
                        save_paired(&paired_path, &now);
                        gate.set_paired(now);
                    }
                }
            });
        }
        let (out, outbox) = unbounded_channel();
        let window = Arc::new(Mutex::new(None));
        runtime.spawn(serve(
            endpoint,
            outbox,
            callbacks,
            identity.fingerprint(),
            paired.clone(),
            paired_path.clone(),
            gate.clone(),
            window.clone(),
        ));
        Ok(HostCore { runtime, out, gate, identity, paired, paired_path, window, frames: Mutex::new(HashMap::new()) })
    }

    fn say(&self, message: HostMessage) {
        let _ = self.out.send(ToSession::Control(message));
    }

    fn video(&self, window: u16, keyframe: bool, captured_us: u64, bytes: Vec<u8>) {
        let frame = self.next_frame(window);
        let _ = self.out.send(ToSession::Video { window, frame, keyframe, captured_us, bytes });
    }

    fn audio(&self, app: String, channels: u16, pcm: Vec<u8>) {
        let _ = self.out.send(ToSession::Audio { app, channels, pcm });
    }

    fn next_frame(&self, window: u16) -> u32 {
        let mut frames = self.frames.lock().unwrap();
        let n = frames.entry(window).or_insert(0);
        *n = n.wrapping_add(1);
        *n
    }
}

// MARK: the listening

async fn arrival(endpoint: &quinn::Endpoint) -> Option<(quinn::Connection, Fingerprint)> {
    loop {
        let incoming = endpoint.accept().await?;
        let connection = match incoming.await {
            Ok(c) => c,
            Err(e) => {
                log::warn!("a session did not get as far as connecting: {e}");
                continue;
            }
        };
        let Some(who) = peer_fingerprint(&connection) else {
            connection.close(1u32.into(), b"no identity");
            continue;
        };
        return Some((connection, who));
    }
}

#[allow(clippy::too_many_arguments)]
async fn serve(
    endpoint: quinn::Endpoint,
    mut outbox: UnboundedReceiver<ToSession>,
    callbacks: Callbacks,
    this_host: Fingerprint,
    paired: Arc<Mutex<Vec<Fingerprint>>>,
    paired_path: PathBuf,
    gate: Arc<Gate>,
    window: Arc<Mutex<Option<Window>>>,
) {
    let mut next: Option<(quinn::Connection, Fingerprint)> = None;
    loop {
        let (connection, who) = match next.take() {
            Some(n) => n,
            None => match arrival(&endpoint).await {
                Some(a) => a,
                None => return,
            },
        };
        let known = paired.lock().unwrap().contains(&who);
        callbacks.say(&json!({ "joined": {
            "who": who.to_string(),
            "short": who.short(),
            "address": connection.remote_address().to_string(),
            "known": known,
        }}));
        // A device not yet trusted is served nothing until it has proved the code shown here.
        let mut control = None;
        if !known {
            match prove(&connection, this_host, who, &window, &gate, callbacks).await {
                Some(stream) => control = Some(stream),
                None => {
                    connection.close(1u32.into(), b"not paired");
                    callbacks.say(&json!({ "left": { "refused": true } }));
                    continue;
                }
            }
            let list = {
                let mut list = paired.lock().unwrap();
                if !list.contains(&who) {
                    list.push(who);
                }
                list.clone()
            };
            save_paired(&paired_path, &list);
            gate.set_paired(list);
            gate.set_open(false);
            spatiand_sync::roster::Roster::open_default().pair(&who.to_string(), "", "");
        }
        // Whatever is queued was meant for the session before this one; this one hears its windows
        // from the start, after its own hello, which the app sends when it hears `attached`.
        while outbox.try_recv().is_ok() {}
        callbacks.say(&json!({ "attached": { "who": who.to_string() } }));
        tokio::select! {
            _ = session(&connection, control, &mut outbox, callbacks) => {}
            _ = gone(&paired, who) => {
                log::info!("{} was removed from the owner's devices", who.short());
                connection.close(1u32.into(), b"not paired");
            }
            arrived = arrival(&endpoint) => {
                let Some(arrived) = arrived else { return };
                connection.close(CLOSE_TAKEN_OVER.into(), b"another device took over");
                next = Some(arrived);
            }
        }
        callbacks.say(&json!({ "left": { "refused": false } }));
    }
}

/// Until this session's device is no longer one of those paired.
async fn gone(paired: &Mutex<Vec<Fingerprint>>, who: Fingerprint) {
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        if !paired.lock().unwrap().contains(&who) {
            return;
        }
    }
}

/// A device not paired yet, through the open gate: it proves the code shown at this Mac or is
/// turned away. `Some` with the control stream the exchange was on -- the session's, from then
/// on -- once it has, and is written down by the caller. What it costs and what it leaves is
/// told to the app as `{"pairing": ...}`.
async fn prove(
    connection: &quinn::Connection,
    this_host: Fingerprint,
    who: Fingerprint,
    window: &Mutex<Option<Window>>,
    gate: &Gate,
    callbacks: Callbacks,
) -> Option<quinn::SendStream> {
    if window.lock().unwrap().is_none() {
        return None;
    }
    let mut control = connection.open_uni().await.ok()?;
    let refuse = |reason: String| HostMessage::Refused { version: spatiand_stream::VERSION, reason };
    let outcome = tokio::time::timeout(PROVING, async {
        loop {
            let mut stream = connection.accept_uni().await.ok()?;
            let bytes = stream.read_to_end(64 * 1024).await.ok()?;
            let Ok(message) = spatiand_stream::from_bytes::<ClientMessage>(&bytes) else { continue };
            match message {
                ClientMessage::PairStart { spake } => {
                    let reply = match window.lock().unwrap().as_mut() {
                        Some(w) => w.start(this_host, who, &spake),
                        None => Err("pairing on this host has closed again".into()),
                    };
                    match reply {
                        Ok(reply) => {
                            let _ = spatiand_stream::link::tell(&mut control, &reply).await;
                        }
                        Err(reason) => {
                            let _ = spatiand_stream::link::tell(&mut control, &refuse(reason)).await;
                            return Some(None);
                        }
                    }
                }
                ClientMessage::PairConfirm { confirm } => {
                    let verdict = match window.lock().unwrap().as_mut() {
                        Some(w) => w.confirm(who, &confirm),
                        None => Verdict::Stray,
                    };
                    match verdict {
                        Verdict::Stray => {}
                        Verdict::Paired(_) => return Some(Some(verdict)),
                        _ => {
                            let wrong = refuse(spatiand_stream::pairing::WRONG_CODE.into());
                            let _ = spatiand_stream::link::tell(&mut control, &wrong).await;
                            return Some(Some(verdict));
                        }
                    }
                }
                // A device from before codes were typed, asking to be let in the old way.
                ClientMessage::Hello { .. } => {
                    let old = refuse(
                        "this host pairs with a code typed on the device; update Spatiand on this \
                         device, then pair again"
                            .into(),
                    );
                    let _ = spatiand_stream::link::tell(&mut control, &old).await;
                    return Some(None);
                }
                _ => {}
            }
        }
    })
    .await;
    // Gone, or out of time, part way through: an attempt that heard the reply is spent.
    let verdict = match outcome {
        Ok(Some(verdict)) => verdict,
        Ok(None) | Err(_) => window.lock().unwrap().as_mut().and_then(|w| w.abandon(&who)),
    };
    match verdict {
        Some(Verdict::Paired(_)) => {
            *window.lock().unwrap() = None;
            callbacks.say(&json!({ "pairing": { "paired": who.short() } }));
            Some(control)
        }
        Some(Verdict::Wrong { left }) => {
            callbacks.say(&json!({ "pairing": { "wrong": { "who": who.short(), "left": left } } }));
            None
        }
        Some(Verdict::Locked) => {
            *window.lock().unwrap() = None;
            gate.set_open(false);
            callbacks.say(&json!({ "pairing": { "locked": true } }));
            None
        }
        Some(Verdict::Stray) | None => None,
    }
}

async fn session(
    connection: &quinn::Connection,
    control: Option<quinn::SendStream>,
    outbox: &mut UnboundedReceiver<ToSession>,
    callbacks: Callbacks,
) {
    // A device that has just paired goes on with the stream its pairing was on: a session reads
    // the first stream the host opens as the control stream.
    let mut control = match control {
        Some(stream) => stream,
        None => match connection.open_uni().await {
            Ok(stream) => stream,
            Err(e) => {
                log::warn!("could not open the control stream: {e}");
                return;
            }
        },
    };
    let reading = tokio::spawn({
        let connection = connection.clone();
        async move {
            while let Ok(stream) = connection.accept_uni().await {
                tokio::spawn(heard(stream, callbacks));
            }
        }
    });
    let mut sounds: HashMap<String, (u16, tokio::sync::mpsc::Sender<Vec<u8>>)> = HashMap::new();
    loop {
        tokio::select! {
            out = outbox.recv() => {
                let Some(out) = out else { break };
                match out {
                    ToSession::Audio { app, channels, pcm } => {
                        let alive = sounds.get(&app).is_some_and(|(width, s)| *width == channels && !s.is_closed());
                        if !alive {
                            let stream = sound_stream(connection, app.clone(), channels);
                            sounds.insert(app.clone(), (channels, stream));
                        }
                        if let Some((_, s)) = sounds.get(&app) {
                            let _ = s.try_send(pcm);
                        }
                    }
                    ToSession::Control(message) => {
                        let Ok(bytes) = spatiand_stream::to_bytes(&message) else { continue };
                        let length = (bytes.len() as u32).to_le_bytes();
                        if control.write_all(&length).await.is_err() || control.write_all(&bytes).await.is_err() {
                            break;
                        }
                    }
                    ToSession::Video { window, frame, keyframe, captured_us, bytes } => {
                        send_picture(connection, window, frame, keyframe, captured_us, &bytes);
                    }
                }
            }
            // Datagrams from the session (a head's pose) are of no use here; read so they do not pile up.
            datagram = connection.read_datagram() => {
                if datagram.is_err() { break }
            }
            _ = connection.closed() => break,
        }
    }
    reading.abort();
}

/// One application's sound, on a stream of its own, opened the first time it plays. A third of a
/// second may wait in the queue; past that the listener would be hearing the past, so what does
/// not fit is dropped.
fn sound_stream(connection: &quinn::Connection, app: String, channels: u16) -> tokio::sync::mpsc::Sender<Vec<u8>> {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(40);
    let connection = connection.clone();
    tokio::spawn(async move {
        let Ok(mut stream) = connection.open_uni().await else { return };
        // A gap in sound is heard and a late picture is only seen.
        let _ = stream.set_priority(10);
        let header = spatiand_stream::audio::AudioHeader {
            app: app.clone(),
            rate: spatiand_stream::audio::RATE,
            channels,
            coding: spatiand_stream::audio::Coding::Pcm,
        };
        if stream.write_all(&header.encode()).await.is_err() {
            return;
        }
        log::info!("sound: sending {app}");
        while let Some(pcm) = rx.recv().await {
            if stream.write_all(&pcm).await.is_err() {
                return;
            }
        }
        let _ = stream.finish();
    });
    tx
}

fn send_picture(connection: &quinn::Connection, window: u16, frame: u32, keyframe: bool, captured_us: u64, bytes: &[u8]) {
    let room = connection.max_datagram_size().unwrap_or(1200).min(1400);
    for (parts, part, chunk) in split(bytes, room) {
        let mut packet = Vec::with_capacity(room);
        Packet {
            window,
            flags: if keyframe { FLAG_KEYFRAME } else { 0 } | if part + 1 == parts { FLAG_LAST } else { 0 },
            frame,
            viewport: 0,
            parts,
            part,
            captured_us,
            payload: chunk,
        }
        .write(&mut packet);
        if connection.send_datagram(packet.into()).is_err() {
            // The congestion controller is full; the next picture is more use than the rest of this one.
            return;
        }
    }
}

/// Read one thing the session opened a stream for.
async fn heard(mut stream: quinn::RecvStream, callbacks: Callbacks) {
    let mut first = Vec::new();
    while first.len() < 8 {
        let mut buffer = [0u8; 8];
        match stream.read(&mut buffer).await {
            Ok(Some(n)) => first.extend_from_slice(&buffer[..n]),
            Ok(None) | Err(_) => break,
        }
    }
    if first.len() >= 8 && first[..4] == spatiand_stream::CONTROL_MAGIC {
        listen_control(stream, first[4..].to_vec(), callbacks).await;
        return;
    }
    // A microphone's sound has nothing to go to here; anything else is one message.
    if first.len() >= 8 && spatiand_stream::audio::AudioHeader::length(&first[..8].try_into().unwrap()).is_some() {
        let mut sink = vec![0u8; 8192];
        while let Ok(Some(_)) = stream.read(&mut sink).await {}
        return;
    }
    if let Ok(rest) = stream.read_to_end(64 * 1024).await {
        first.extend_from_slice(&rest);
        if let Ok(message) = spatiand_stream::from_bytes::<ClientMessage>(&first) {
            callbacks.say(&to_app_json(&message));
        }
    }
}

async fn listen_control(mut stream: quinn::RecvStream, rest: Vec<u8>, callbacks: Callbacks) {
    const LARGEST: usize = 1 << 20;
    let mut held = rest;
    let mut buffer = vec![0u8; 16 * 1024];
    loop {
        loop {
            if held.len() < 4 {
                break;
            }
            let length = u32::from_le_bytes(held[..4].try_into().unwrap()) as usize;
            if length > LARGEST {
                return;
            }
            if held.len() < 4 + length {
                break;
            }
            if let Ok(message) = spatiand_stream::from_bytes::<ClientMessage>(&held[4..4 + length]) {
                callbacks.say(&to_app_json(&message));
            }
            held.drain(..4 + length);
        }
        match stream.read(&mut buffer).await {
            Ok(Some(n)) => held.extend_from_slice(&buffer[..n]),
            Ok(None) | Err(_) => return,
        }
    }
}

// MARK: the C interface

/// Start listening. Null if it could not.
#[no_mangle]
pub extern "C" fn sp_host_start(identity_dir: *const c_char, port: u16, user: *mut c_void, on_event: EventFn) -> *mut HostCore {
    if identity_dir.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: a NUL-terminated string, by the contract.
    let dir = PathBuf::from(unsafe { CStr::from_ptr(identity_dir) }.to_string_lossy().into_owned());
    match HostCore::start(dir, port, Callbacks { user: user as usize, event: on_event }) {
        Ok(core) => Box::into_raw(Box::new(core)),
        Err(e) => {
            log::error!("could not start the host: {e}");
            std::ptr::null_mut()
        }
    }
}

#[no_mangle]
pub extern "C" fn sp_host_stop(core: *mut HostCore) {
    if !core.is_null() {
        // SAFETY: from `sp_host_start`, freed once.
        let core = unsafe { Box::from_raw(core) };
        core.runtime.shutdown_background();
    }
}

/// This host's fingerprint, in full. Free with `sp_free_string`.
#[no_mangle]
pub extern "C" fn sp_host_fingerprint(core: *mut HostCore) -> *mut c_char {
    // SAFETY: from `sp_host_start`, or null.
    let Some(core) = (unsafe { core.as_ref() }) else { return std::ptr::null_mut() };
    CString::new(core.identity.fingerprint().to_string()).map_or(std::ptr::null_mut(), CString::into_raw)
}

/// Say something to the session: a `HostMessage` as JSON.
#[no_mangle]
pub extern "C" fn sp_host_say(core: *mut HostCore, json: *const c_char) {
    // SAFETY: from `sp_host_start`, or null; and a NUL-terminated string.
    let (Some(core), false) = (unsafe { core.as_ref() }, json.is_null()) else { return };
    let text = unsafe { CStr::from_ptr(json) }.to_string_lossy().into_owned();
    match from_app_json(&text) {
        Ok(message) => core.say(message),
        Err(e) => log::warn!("the app said something that is not a host message: {e}"),
    }
}

/// One picture of one window: an Annex B access unit, parameter sets in front of a keyframe.
#[no_mangle]
pub extern "C" fn sp_host_video(core: *mut HostCore, window: u16, keyframe: i32, captured_us: u64, data: *const u8, length: usize) {
    // SAFETY: as above, and `length` readable bytes.
    let (Some(core), false) = (unsafe { core.as_ref() }, data.is_null()) else { return };
    let bytes = unsafe { std::slice::from_raw_parts(data, length) }.to_vec();
    core.video(window, keyframe != 0, captured_us, bytes);
}

/// A little of one application's sound: signed 16-bit little-endian samples, interleaved, at 48 kHz.
#[no_mangle]
pub extern "C" fn sp_host_audio(core: *mut HostCore, app: *const c_char, channels: u16, data: *const u8, length: usize) {
    // SAFETY: as above, a NUL-terminated name and `length` readable bytes.
    let (Some(core), false, false) = (unsafe { core.as_ref() }, data.is_null(), app.is_null()) else { return };
    let app = unsafe { CStr::from_ptr(app) }.to_string_lossy().into_owned();
    core.audio(app, channels, unsafe { std::slice::from_raw_parts(data, length) }.to_vec());
}

/// Let a device that has never been here pair, or stop.
///
/// Opening makes a fresh code and returns it, `482913`, for the app to show at this Mac -- and
/// only there: it is never sent. The device's user types it; a device that proves it is let in
/// and written down, with nobody asked to say yes (`spatiand_stream::pairing`). Free the code
/// with `sp_free_string`. Closing, or a code that could not be made, returns null.
#[no_mangle]
pub extern "C" fn sp_host_open_pairing(core: *mut HostCore, open: i32) -> *mut c_char {
    // SAFETY: from `sp_host_start`, or null.
    let Some(core) = (unsafe { core.as_ref() }) else { return std::ptr::null_mut() };
    if open == 0 {
        *core.window.lock().unwrap() = None;
        core.gate.set_open(false);
        return std::ptr::null_mut();
    }
    let window = match Window::open(spatiand_stream::pairing::WINDOW) {
        Ok(window) => window,
        Err(e) => {
            log::error!("{e}");
            return std::ptr::null_mut();
        }
    };
    let code = CString::new(window.code()).ok();
    *core.window.lock().unwrap() = Some(window);
    core.gate.set_open(true);
    code.map(CString::into_raw).unwrap_or(std::ptr::null_mut())
}

/// Trust a device by its fingerprint, without it having to ask: for pairing in the other direction.
#[no_mangle]
pub extern "C" fn sp_host_trust(core: *mut HostCore, fingerprint: *const c_char) {
    // SAFETY: as above.
    let (Some(core), false) = (unsafe { core.as_ref() }, fingerprint.is_null()) else { return };
    let text = unsafe { CStr::from_ptr(fingerprint) }.to_string_lossy().into_owned();
    if let Ok(who) = text.parse::<Fingerprint>() {
        let list = {
            let mut list = core.paired.lock().unwrap();
            if !list.contains(&who) {
                list.push(who);
            }
            list.clone()
        };
        save_paired(&core.paired_path, &list);
        core.gate.set_paired(list);
    }
}

/// How many devices are paired with this host.
#[no_mangle]
pub extern "C" fn sp_host_paired_count(core: *mut HostCore) -> i32 {
    // SAFETY: from `sp_host_start`, or null.
    unsafe { core.as_ref() }.map_or(0, |c| c.paired.lock().unwrap().len() as i32)
}

/// Forget every paired device.
#[no_mangle]
pub extern "C" fn sp_host_forget_all(core: *mut HostCore) {
    // SAFETY: from `sp_host_start`, or null.
    if let Some(core) = unsafe { core.as_ref() } {
        core.paired.lock().unwrap().clear();
        save_paired(&core.paired_path, &[]);
        core.gate.set_paired(Vec::new());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_catalogue_with_an_icon_crosses_as_json_and_comes_back_a_host_message() {
        let icon = base64::engine::general_purpose::STANDARD.encode([137u8, 80, 78, 71]);
        let text = json!({ "Catalog": { "apps": [
            { "id": "com.apple.Safari", "name": "Safari", "exec": "open", "icon_png": icon }
        ]}})
        .to_string();
        let message = from_app_json(&text).expect("a catalogue");
        match message {
            HostMessage::Catalog { apps } => {
                assert_eq!(apps.len(), 1);
                assert_eq!(apps[0].icon_png.as_deref(), Some(&[137u8, 80, 78, 71][..]));
            }
            other => panic!("not a catalogue: {other:?}"),
        }
    }

    #[test]
    fn a_window_opening_is_written_the_way_the_session_reads_it() {
        let text = json!({ "Opened": { "window": 3, "app": "Safari", "title": "Home", "width": 1280, "height": 800, "parent": null }}).to_string();
        assert!(matches!(from_app_json(&text), Ok(HostMessage::Opened(_))));
    }

    #[test]
    fn what_a_session_says_reaches_the_app_with_bytes_as_base64() {
        let said = ClientMessage::Clipboard(spatiand_stream::control::Clipboard::Data { mime_type: "image/png".into(), bytes: vec![1, 2, 3] });
        let json = to_app_json(&said);
        assert_eq!(json.pointer("/said/Clipboard/Data/bytes").and_then(Value::as_str), Some("AQID"));
    }

    #[test]
    fn the_paired_list_survives_a_restart() {
        let dir = std::env::temp_dir().join(format!("spatiand-paired-{}", std::process::id()));
        let path = dir.join("host-paired.txt");
        let who = Fingerprint([7; 32]);
        save_paired(&path, &[who]);
        assert_eq!(load_paired(&path), vec![who]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_stream_announcement_is_understood() {
        let text = json!({ "Stream": { "window": 1, "codec": "H265", "width": 594, "height": 421, "eyes": "mono" }}).to_string();
        let message = from_app_json(&text);
        assert!(matches!(message, Ok(HostMessage::Stream { .. })), "{message:?}");
    }
}
