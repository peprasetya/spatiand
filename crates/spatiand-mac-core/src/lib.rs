//! The link to a Spatiand host, behind a small C interface, for the Mac app.
//!
//! The Mac app is Swift and the link -- QUIC, pairing, framing, the clipboard's rules -- is the
//! Rust the Deck and the hosts already share, so this is the seam between them and nothing more:
//! it opens a session, hands pictures and sound up as bytes, and carries every other message as
//! JSON so that the Swift side can grow without this side being rebuilt for each new variant.
//! `spatiand_core.h` is the interface; this file is what is behind it.
//!
//! It is `probe-session` made into a library: the same Hello, the same control stream, the same
//! reassembly of frames, with callbacks where the probe printed.

pub mod host;

use std::ffi::{c_char, c_void, CStr, CString};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use serde_json::{json, Value};
use spatiand_stream::video::{Arrival, Packet, Reassembler};
use spatiand_stream::{link, ClientMessage, Codec, Fingerprint, HostMessage, Identity, WindowId};
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};

type EventFn = extern "C" fn(*mut c_void, *const c_char);
type VideoFn = extern "C" fn(*mut c_void, u16, i32, i32, u64, *const u8, usize);
type AudioFn = extern "C" fn(*mut c_void, *const c_char, u16, *const u8, usize);

/// What to call, and who to say it to. The pointer is the caller's and is only ever handed back.
#[derive(Clone, Copy)]
struct Callbacks {
    user: usize,
    event: EventFn,
    video: VideoFn,
    audio: AudioFn,
}

impl Callbacks {
    fn say(&self, value: &Value) {
        if let Ok(text) = CString::new(value.to_string()) {
            (self.event)(self.user as *mut c_void, text.as_ptr());
        }
    }
    fn core(&self, what: &str, detail: Value) {
        self.say(&json!({ "core": { what: detail } }));
    }
}

pub struct Core {
    runtime: tokio::runtime::Runtime,
    identity: Arc<Identity>,
    callbacks: Callbacks,
    session: Mutex<Option<UnboundedSender<ClientMessage>>>,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

/// A host message as JSON for the Swift side.
///
/// The one change from what serde writes: bytes -- an application's icon (a PNG), a pasted image
/// -- or a cursor's pixels -- are as a JSON array of numbers ten times their size and slow to read. They go as base64,
/// and [`client_json`] reads them back the same way.
pub fn host_json(message: &HostMessage) -> Value {
    let mut value = serde_json::to_value(message).unwrap_or(Value::Null);
    fn icons(value: &mut Value) {
        match value {
            Value::Object(map) => {
                for (key, inner) in map.iter_mut() {
                    if key == "icon_png" || key == "bytes" || key == "pixels" {
                        if let Value::Array(bytes) = inner {
                            let raw: Vec<u8> =
                                bytes.iter().filter_map(|b| b.as_u64().map(|b| b as u8)).collect();
                            *inner = Value::String(
                                base64::engine::general_purpose::STANDARD.encode(raw),
                            );
                        }
                    } else {
                        icons(inner);
                    }
                }
            }
            Value::Array(items) => items.iter_mut().for_each(icons),
            _ => {}
        }
    }
    icons(&mut value);
    json!({ "host": value })
}

/// What the Swift side writes, put back as serde reads it: the one place a message carries bytes
/// from this end -- the answer to a paste -- arrives as base64 and goes on as an array.
pub fn client_json(text: &str) -> Result<ClientMessage, String> {
    let mut value: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    if let Some(data) = value.pointer_mut("/Clipboard/Data/bytes") {
        if let Value::String(encoded) = data {
            let raw = base64::engine::general_purpose::STANDARD
                .decode(encoded.as_bytes())
                .map_err(|e| e.to_string())?;
            *data = Value::Array(raw.into_iter().map(|b| Value::from(b)).collect());
        }
    }
    serde_json::from_value(value).map_err(|e| e.to_string())
}

fn codec_number(codec: Codec) -> i32 {
    match codec {
        Codec::H264 => 0,
        Codec::H265 => 1,
        Codec::Av1 => 2,
    }
}

impl Core {
    fn connect(&self, address: String, fingerprint: String) {
        let Ok(host) = fingerprint.parse::<Fingerprint>() else {
            self.callbacks
                .core("disconnected", json!("that is not a host fingerprint"));
            return;
        };
        self.disconnect();
        let (tx, rx) = unbounded_channel();
        *self.session.lock().unwrap() = Some(tx);
        let identity = self.identity.clone();
        let callbacks = self.callbacks;
        let handle = self.runtime.spawn(async move {
            let ended = run_session(identity, address, host, callbacks, rx).await;
            callbacks.core("disconnected", json!(ended.unwrap_or_else(|e| e)));
        });
        *self.task.lock().unwrap() = Some(handle);
    }

    fn disconnect(&self) {
        if let Some(tx) = self.session.lock().unwrap().take() {
            let _ = tx.send(ClientMessage::Detach);
        }
        if let Some(task) = self.task.lock().unwrap().take() {
            // A moment for the Detach to leave, then let go of whatever is left.
            let _ = self.runtime.block_on(async {
                tokio::time::timeout(Duration::from_millis(300), task).await
            });
        }
    }
}

async fn run_session(
    identity: Arc<Identity>,
    address: String,
    host: Fingerprint,
    callbacks: Callbacks,
    mut outgoing: tokio::sync::mpsc::UnboundedReceiver<ClientMessage>,
) -> Result<String, String> {
    let endpoint = spatiand_stream::transport::client(&identity, host)?;
    let target = link::resolve(&address).await?;
    let connecting = endpoint
        .connect(target, "spatiand")
        .map_err(|e| format!("cannot reach {address}: {e}"))?;
    let connection = tokio::time::timeout(Duration::from_secs(10), connecting)
        .await
        .map_err(|_| format!("{address} did not answer"))?
        .map_err(|e| format!("{address} would not connect: {e}"))?;

    link::say(
        &connection,
        &ClientMessage::Hello {
            version: spatiand_stream::VERSION,
            // Both, best first: the Mac decodes either in hardware.
            codecs: vec![Codec::H265, Codec::H264],
            max_size: (3840, 2160),
            refresh_mhz: 60_000,
            session: String::new(),
        },
    )
    .await?;
    callbacks.core("connected", json!(address));

    // Whatever the app says goes out in the order it was said.
    let sender = connection.clone();
    tokio::spawn(async move {
        while let Some(message) = outgoing.recv().await {
            let detach = matches!(message, ClientMessage::Detach);
            if link::say(&sender, &message).await.is_err() || detach {
                break;
            }
        }
    });

    let control = connection
        .accept_uni()
        .await
        .map_err(|e| format!("the host sent no control stream: {e}"))?;
    let mut control = Box::pin(link::hear(control));
    let mut windows: std::collections::HashMap<u16, Reassembler> = Default::default();
    let mut codecs: std::collections::HashMap<u16, Codec> = Default::default();

    loop {
        tokio::select! {
            heard = &mut control => {
                let Some((message, rest)) = heard else { return Ok(why(&connection, "the host closed the session")) };
                if let HostMessage::Stream { window, codec, .. } = &message {
                    codecs.insert(window.0 as u16, *codec);
                }
                if let HostMessage::Closed { window } = &message {
                    windows.remove(&(window.0 as u16));
                }
                callbacks.say(&host_json(&message));
                control = Box::pin(link::hear(rest));
            }
            incoming = connection.accept_uni() => {
                let Ok(stream) = incoming else { return Ok(why(&connection, "the connection closed")) };
                tokio::spawn(receive_sound(stream, callbacks));
            }
            datagram = connection.read_datagram() => {
                let Ok(datagram) = datagram else { return Ok(why(&connection, "the connection closed")) };
                let Some(packet) = Packet::read(&datagram) else { continue };
                let window = packet.window;
                match windows.entry(window).or_default().accept(&packet) {
                    Arrival::Frame(frame) => {
                        let codec = codecs.get(&window).copied().unwrap_or(Codec::H265);
                        (callbacks.video)(
                            callbacks.user as *mut c_void,
                            window,
                            codec_number(codec),
                            frame.keyframe as i32,
                            frame.captured_us,
                            frame.bytes.as_ptr(),
                            frame.bytes.len(),
                        );
                    }
                    Arrival::Lost(_) => {
                        let _ = link::say(&connection, &ClientMessage::WantKeyframe {
                            window: WindowId(window as u32),
                        }).await;
                    }
                    Arrival::Partial | Arrival::Stale => {}
                }
            }
        }
    }
}

/// Why a session ended, in words: and when the host closed it because another device took its
/// windows, the one fixed sentence [`TAKEN_OVER`], which the app knows not to answer by calling
/// straight back.
fn why(connection: &quinn::Connection, otherwise: &str) -> String {
    match connection.close_reason() {
        Some(quinn::ConnectionError::ApplicationClosed(ref closed))
            if closed.error_code == spatiand_stream::transport::CLOSE_TAKEN_OVER.into() =>
        {
            TAKEN_OVER.into()
        }
        _ => otherwise.into(),
    }
}

/// What `disconnected` says when another device took the host's windows over.
const TAKEN_OVER: &str = "taken over";

async fn receive_sound(mut stream: quinn::RecvStream, callbacks: Callbacks) {
    use spatiand_stream::audio::{frame_bytes, is_supported, AudioHeader};
    let mut first = [0u8; 8];
    if stream.read_exact(&mut first).await.is_err() {
        return;
    }
    let Some(length) = AudioHeader::length(&first) else { return };
    let mut body = vec![0u8; length];
    if stream.read_exact(&mut body).await.is_err() {
        return;
    }
    let Some(header) = AudioHeader::decode(&body) else { return };
    if !is_supported(header.channels) {
        return;
    }
    let (Ok(app), frame) = (CString::new(header.app.clone()), frame_bytes(header.channels)) else {
        return;
    };
    let mut buffer = vec![0u8; 8192];
    // Whole frames only: half of one would put every channel after it one place out.
    let mut carry: Vec<u8> = Vec::new();
    while let Ok(Some(n)) = stream.read(&mut buffer).await {
        carry.extend_from_slice(&buffer[..n]);
        let whole = carry.len() / frame * frame;
        if whole > 0 {
            (callbacks.audio)(
                callbacks.user as *mut c_void,
                app.as_ptr(),
                header.channels,
                carry.as_ptr(),
                whole,
            );
            carry.drain(..whole);
        }
    }
}

fn string(pointer: *const c_char) -> Option<String> {
    if pointer.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(pointer) }.to_str().ok().map(str::to_owned)
}

/// # Safety
/// Pointers as `spatiand_core.h` describes them.
#[no_mangle]
pub unsafe extern "C" fn sp_start(
    identity_dir: *const c_char,
    user: *mut c_void,
    on_event: EventFn,
    on_video: VideoFn,
    on_audio: AudioFn,
) -> *mut Core {
    let Some(dir) = string(identity_dir) else { return std::ptr::null_mut() };
    let Ok(identity) = Identity::load_or_create(&PathBuf::from(dir)) else {
        return std::ptr::null_mut();
    };
    let Ok(runtime) = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .thread_name("spatiand-link")
        .build()
    else {
        return std::ptr::null_mut();
    };
    Box::into_raw(Box::new(Core {
        runtime,
        identity: Arc::new(identity),
        callbacks: Callbacks { user: user as usize, event: on_event, video: on_video, audio: on_audio },
        session: Mutex::new(None),
        task: Mutex::new(None),
    }))
}

/// # Safety
/// `core` from `sp_start`.
#[no_mangle]
pub unsafe extern "C" fn sp_fingerprint(core: *mut Core) -> *mut c_char {
    let Some(core) = core.as_ref() else { return std::ptr::null_mut() };
    CString::new(core.identity.fingerprint().to_string())
        .map(CString::into_raw)
        .unwrap_or(std::ptr::null_mut())
}

/// # Safety
/// `core` from `sp_start`, `address` a C string.
#[no_mangle]
pub unsafe extern "C" fn sp_pair(core: *mut Core, address: *const c_char) {
    let (Some(core), Some(address)) = (core.as_ref(), string(address)) else { return };
    let identity = core.identity.clone();
    let callbacks = core.callbacks;
    core.runtime.spawn(async move {
        let outcome = link::pair(&identity, &address, |p| {
            let link::Pairing::Compare { fingerprint, code } = p;
            callbacks.core(
                "compare",
                json!({ "fingerprint": fingerprint.to_string(), "code": code }),
            );
        })
        .await;
        match outcome {
            Ok(paired) => callbacks.core(
                "paired",
                json!({ "name": paired.name, "fingerprint": paired.fingerprint.to_string(), "address": address }),
            ),
            Err(e) => callbacks.core("pair_failed", json!(e)),
        }
    });
}

/// # Safety
/// `core` from `sp_start`, strings as C strings.
#[no_mangle]
pub unsafe extern "C" fn sp_connect(core: *mut Core, address: *const c_char, fingerprint: *const c_char) {
    let (Some(core), Some(address), Some(fingerprint)) =
        (core.as_ref(), string(address), string(fingerprint))
    else {
        return;
    };
    core.connect(address, fingerprint);
}

/// # Safety
/// `core` from `sp_start`, `message` a C string of JSON.
#[no_mangle]
pub unsafe extern "C" fn sp_say(core: *mut Core, message: *const c_char) {
    let (Some(core), Some(text)) = (core.as_ref(), string(message)) else { return };
    match client_json(&text) {
        Ok(message) => {
            if let Some(tx) = core.session.lock().unwrap().as_ref() {
                let _ = tx.send(message);
            }
        }
        Err(e) => log::warn!("spatiand core: not a client message ({e}): {text}"),
    }
}

/// # Safety
/// `core` from `sp_start`.
#[no_mangle]
pub unsafe extern "C" fn sp_disconnect(core: *mut Core) {
    if let Some(core) = core.as_ref() {
        core.disconnect();
    }
}

/// # Safety
/// A string this library returned.
#[no_mangle]
pub unsafe extern "C" fn sp_free_string(string: *mut c_char) {
    if !string.is_null() {
        drop(CString::from_raw(string));
    }
}

/// # Safety
/// `core` from `sp_start`, not used again.
#[no_mangle]
pub unsafe extern "C" fn sp_stop(core: *mut Core) {
    if !core.is_null() {
        let core = Box::from_raw(core);
        core.disconnect();
        core.runtime.shutdown_background();
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use spatiand_stream::{App, Catalog};

    #[test]
    fn an_icon_travels_as_base64_and_not_as_ten_times_its_size_in_numbers() {
        let mut app = App::new("chrome", "Chrome", "/usr/bin/chrome");
        app.icon_png = Some(vec![137, 80, 78, 71]);
        let message = HostMessage::Catalog { apps: Catalog { apps: vec![app] }.apps };
        let value = host_json(&message);
        let icon = &value["host"]["Catalog"]["apps"][0]["icon_png"];
        assert_eq!(icon, &json!("iVBORw=="));
    }

    #[test]
    fn a_pasted_image_crosses_as_base64_both_ways() {
        let data = ClientMessage::Clipboard(spatiand_stream::control::Clipboard::Data {
            mime_type: "image/png".into(),
            bytes: vec![137, 80, 78, 71, 13, 10],
        });
        // As the host would say it to us: bytes as base64.
        let said = host_json(&HostMessage::Clipboard(spatiand_stream::control::Clipboard::Data {
            mime_type: "image/png".into(),
            bytes: vec![137, 80, 78, 71, 13, 10],
        }));
        assert_eq!(said["host"]["Clipboard"]["Data"]["bytes"], json!("iVBORw0K"));
        // And as we say it to the host.
        let text = r#"{"Clipboard":{"Data":{"mime_type":"image/png","bytes":"iVBORw0K"}}}"#;
        assert_eq!(client_json(text).unwrap(), data);
    }

    #[test]
    fn a_client_message_comes_back_from_json_as_what_it_was() {
        let message = ClientMessage::Launch { app: "settings".into() };
        let text = serde_json::to_string(&message).unwrap();
        assert_eq!(serde_json::from_str::<ClientMessage>(&text).unwrap(), message);
        // As the Swift side writes one.
        let by_hand = r#"{"Input":{"window":3,"input":{"Motion":{"x":10.5,"y":20.0}}}}"#;
        assert!(matches!(
            serde_json::from_str::<ClientMessage>(by_hand),
            Ok(ClientMessage::Input { .. })
        ));
    }
}
