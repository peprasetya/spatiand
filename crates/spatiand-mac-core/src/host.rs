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
use spatiand_stream::{pairing_code, ClientMessage, Fingerprint, Gate, HostMessage, Identity};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;

type EventFn = extern "C" fn(*mut c_void, *const c_char);

/// How long a stranger has to be answered, before it is turned away.
const DECISION: Duration = Duration::from_secs(90);

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
}

pub struct HostCore {
    runtime: tokio::runtime::Runtime,
    out: UnboundedSender<ToSession>,
    gate: Arc<Gate>,
    identity: Arc<Identity>,
    paired: Arc<Mutex<Vec<Fingerprint>>>,
    paired_path: PathBuf,
    /// Who is waiting to be let in, and the way to answer them.
    waiting: Arc<Mutex<Option<oneshot::Sender<bool>>>>,
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
        let (out, outbox) = unbounded_channel();
        let waiting = Arc::new(Mutex::new(None));
        runtime.spawn(serve(
            endpoint,
            outbox,
            callbacks,
            identity.fingerprint(),
            paired.clone(),
            paired_path.clone(),
            gate.clone(),
            waiting.clone(),
        ));
        Ok(HostCore { runtime, out, gate, identity, paired, paired_path, waiting, frames: Mutex::new(HashMap::new()) })
    }

    fn say(&self, message: HostMessage) {
        let _ = self.out.send(ToSession::Control(message));
    }

    fn video(&self, window: u16, keyframe: bool, captured_us: u64, bytes: Vec<u8>) {
        let frame = self.next_frame(window);
        let _ = self.out.send(ToSession::Video { window, frame, keyframe, captured_us, bytes });
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
    waiting: Arc<Mutex<Option<oneshot::Sender<bool>>>>,
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
            "code": pairing_code(&this_host, &who),
            "known": known,
        }}));
        // A device not yet trusted is served nothing until the owner has said yes.
        if !known {
            let (tx, rx) = oneshot::channel();
            *waiting.lock().unwrap() = Some(tx);
            let answer = tokio::time::timeout(DECISION, rx).await;
            *waiting.lock().unwrap() = None;
            if !matches!(answer, Ok(Ok(true))) {
                connection.close(1u32.into(), b"not paired");
                callbacks.say(&json!({ "left": { "refused": true } }));
                continue;
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
        }
        tokio::select! {
            _ = session(&connection, &mut outbox, callbacks) => {}
            arrived = arrival(&endpoint) => {
                let Some(arrived) = arrived else { return };
                connection.close(CLOSE_TAKEN_OVER.into(), b"another device took over");
                next = Some(arrived);
            }
        }
        callbacks.say(&json!({ "left": { "refused": false } }));
    }
}

async fn session(connection: &quinn::Connection, outbox: &mut UnboundedReceiver<ToSession>, callbacks: Callbacks) {
    // Whatever is queued was meant for the session before this one.
    while outbox.try_recv().is_ok() {}
    let mut control = match connection.open_uni().await {
        Ok(stream) => stream,
        Err(e) => {
            log::warn!("could not open the control stream: {e}");
            return;
        }
    };
    let reading = tokio::spawn({
        let connection = connection.clone();
        async move {
            while let Ok(stream) = connection.accept_uni().await {
                tokio::spawn(heard(stream, callbacks));
            }
        }
    });
    loop {
        tokio::select! {
            out = outbox.recv() => {
                let Some(out) = out else { break };
                match out {
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

/// Let a device that has never been here pair, for as long as this is on, or stop.
#[no_mangle]
pub extern "C" fn sp_host_open_pairing(core: *mut HostCore, open: i32) {
    // SAFETY: from `sp_host_start`, or null.
    if let Some(core) = unsafe { core.as_ref() } {
        core.gate.set_open(open != 0);
    }
}

/// The owner's answer about the device that is waiting to be let in.
#[no_mangle]
pub extern "C" fn sp_host_decide(core: *mut HostCore, admit: i32) {
    // SAFETY: from `sp_host_start`, or null.
    if let Some(core) = unsafe { core.as_ref() } {
        if let Some(tx) = core.waiting.lock().unwrap().take() {
            let _ = tx.send(admit != 0);
        }
    }
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
}
