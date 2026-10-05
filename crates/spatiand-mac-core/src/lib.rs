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

pub mod chrome;
pub mod host;
pub mod logging;
pub mod menu_model;
pub mod look;
pub mod pads;
pub mod room;
pub mod shell_ui;

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


// MARK: the room

/// The glasses' world, behind a lock: the glasses' sensor thread feeds it and the drawing thread
/// reads it. See [`room`].
pub struct RoomHandle(Mutex<room::Room>);

#[repr(C)]
pub struct SpAim {
    /// The window's id, or -1 for none.
    pub window: i32,
    pub x: f64,
    pub y: f64,
    pub point: [f64; 3],
    /// What part of the window: 0 nothing, 1 its surface, 2 title bar, 3 close, 4 hide, 5 pin, 6 speaker,
    /// 7 left edge, 8 right edge, 9 bottom edge, 10 bottom left corner, 11 bottom right corner.
    pub zone: i32,
}

#[repr(C)]
pub struct SpDraw {
    pub window: u32,
    pub first: u32,
    pub count: u32,
    /// Bit 0 focused, bit 1 aimed at, bit 2 pinned to the glass.
    pub flags: u32,
}

fn with_room<R>(room: *mut RoomHandle, default: R, f: impl FnOnce(&mut room::Room) -> R) -> R {
    // SAFETY: the pointer came from `sp_room_new` and has not been freed, or it is null.
    match unsafe { room.as_ref() } {
        Some(handle) => f(&mut handle.0.lock().unwrap()),
        None => default,
    }
}

#[no_mangle]
pub extern "C" fn sp_room_new() -> *mut RoomHandle {
    logging::init();
    Box::into_raw(Box::new(RoomHandle(Mutex::new(room::Room::new()))))
}

#[no_mangle]
pub extern "C" fn sp_room_free(room: *mut RoomHandle) {
    if !room.is_null() {
        // SAFETY: from `sp_room_new`, freed once.
        drop(unsafe { Box::from_raw(room) });
    }
}

/// One sample from the glasses: angular rate in degrees a second, acceleration in g, field in gauss.
#[no_mangle]
pub extern "C" fn sp_room_imu(room: *mut RoomHandle, timestamp_ns: u64, gyro: *const f64, accel: *const f64, mag: *const f64) {
    if gyro.is_null() || accel.is_null() || mag.is_null() {
        return;
    }
    // SAFETY: three readable arrays of three, by the contract.
    let (g, a, m) = unsafe { (std::slice::from_raw_parts(gyro, 3), std::slice::from_raw_parts(accel, 3), std::slice::from_raw_parts(mag, 3)) };
    let sample = spatiand_hmd::ImuSample {
        timestamp_ns,
        gyro: glam::DVec3::new(g[0], g[1], g[2]),
        accel: glam::DVec3::new(a[0], a[1], a[2]),
        mag: glam::DVec3::new(m[0], m[1], m[2]),
        temperature_c: None,
    };
    with_room(room, (), |r| r.imu(&sample));
}

/// Which glasses these are, by the name the Deck knows them by ("XREAL Air"): what the tracker's
/// remembered sensor calibration is kept under.
#[no_mangle]
pub extern "C" fn sp_room_set_device(room: *mut RoomHandle, name: *const c_char) {
    if name.is_null() {
        return;
    }
    // SAFETY: a NUL-terminated string, by the contract.
    let name = unsafe { CStr::from_ptr(name) }.to_string_lossy().into_owned();
    with_room(room, (), |r| r.set_device(&name));
}

/// Both eyes' matrices for drawing the environment: 32 floats, left then right.
#[no_mangle]
pub extern "C" fn sp_room_sky_matrices(room: *mut RoomHandle, out: *mut f32) {
    if out.is_null() {
        return;
    }
    let [l, r] = with_room(room, [glam::Mat4::IDENTITY; 2], |r| r.sky_matrices());
    // SAFETY: room for 32 floats, by the contract.
    let out = unsafe { std::slice::from_raw_parts_mut(out, 32) };
    out[..16].copy_from_slice(&l.to_cols_array());
    out[16..].copy_from_slice(&r.to_cols_array());
}

/// The studio the Deck starts in, generated: a dark sky with a key light, a horizon and a floor
/// grid. RGBA, `width * height * 4` bytes, equirectangular, top row first. Without it the room is a
/// black void and nothing says how far away anything is.
#[no_mangle]
pub extern "C" fn sp_sky_studio(width: u32, height: u32, out: *mut u8) {
    if out.is_null() || width == 0 || height == 0 {
        return;
    }
    let sky = spatiand_render::Sky::studio(width, height);
    // SAFETY: room for `width * height * 4` bytes, by the contract.
    let out = unsafe { std::slice::from_raw_parts_mut(out, (width as usize) * (height as usize) * 4) };
    let n = out.len().min(sky.rgba.len());
    out[..n].copy_from_slice(&sky.rgba[..n]);
}

#[no_mangle]
pub extern "C" fn sp_room_recentre(room: *mut RoomHandle) {
    with_room(room, (), |r| r.recentre());
}

#[no_mangle]
pub extern "C" fn sp_room_has_head(room: *mut RoomHandle) -> i32 {
    with_room(room, 0, |r| r.has_head() as i32)
}

/// For the preview, where there are no sensors: hold the head at this heading and pitch, in degrees.
#[no_mangle]
pub extern "C" fn sp_room_set_head(room: *mut RoomHandle, enable: i32, yaw_deg: f64, pitch_deg: f64) {
    use glam::{DQuat, DVec3};
    with_room(room, (), |r| {
        r.set_fixed_head((enable != 0).then(|| {
            DQuat::from_axis_angle(DVec3::Z, yaw_deg.to_radians()) * DQuat::from_axis_angle(DVec3::Y, -pitch_deg.to_radians())
        }))
    });
}

/// The head's heading, pitch and roll, degrees, into three doubles.
#[no_mangle]
pub extern "C" fn sp_room_head_euler(room: *mut RoomHandle, out: *mut f64) {
    let (y, p, r) = with_room(room, (0.0, 0.0, 0.0), |r| r.euler_degrees());
    if !out.is_null() {
        // SAFETY: three writable doubles, by the contract.
        unsafe { std::slice::from_raw_parts_mut(out, 3).copy_from_slice(&[y, p, r]) };
    }
}

#[no_mangle]
pub extern "C" fn sp_room_set_per_eye(room: *mut RoomHandle, width: u32, height: u32) {
    with_room(room, (), |r| r.set_per_eye(width, height));
}

#[no_mangle]
pub extern "C" fn sp_room_set_window(room: *mut RoomHandle, id: u32, width: u32, height: u32) {
    with_room(room, (), |r| r.set_window(id, (width, height)));
}

/// A panel of Spatiand's own, like the menu, `width` metres across at `radius` metres, put where
/// the wearer is looking. Ids from 0xFFF0 up. Its picture is the app's to draw.
#[no_mangle]
pub extern "C" fn sp_room_set_panel(room: *mut RoomHandle, id: u32, width_px: u32, height_px: u32, width_m: f64, radius_m: f64) {
    with_room(room, (), |r| r.set_panel(id, (width_px, height_px), width_m, radius_m));
}

/// Say which application a window belongs to, so its sound is put where the window is.
#[no_mangle]
pub extern "C" fn sp_room_set_app(room: *mut RoomHandle, id: u32, app: *const c_char) {
    if app.is_null() {
        return;
    }
    // SAFETY: a NUL-terminated string, by the contract.
    let app = unsafe { CStr::from_ptr(app) }.to_string_lossy().into_owned();
    with_room(room, (), |r| r.set_app(id, &app));
}

/// An application's sound, placed in the room and folded to two ears. `pcm` is signed 16-bit
/// little-endian, `channels` interleaved. Writes interleaved stereo floats into `out`, which
/// has room for `capacity` floats, and returns how many; 0 if it would not fit.
#[no_mangle]
pub extern "C" fn sp_room_audio(
    room: *mut RoomHandle,
    app: *const c_char,
    channels: u16,
    pcm: *const u8,
    length: usize,
    out: *mut f32,
    capacity: usize,
) -> usize {
    if app.is_null() || pcm.is_null() || out.is_null() {
        return 0;
    }
    // SAFETY: a NUL-terminated string and buffers of the lengths given, by the contract.
    let (app, pcm) = unsafe { (CStr::from_ptr(app).to_string_lossy().into_owned(), std::slice::from_raw_parts(pcm, length)) };
    let mut rendered = Vec::new();
    with_room(room, (), |r| r.render_audio(&app, channels as usize, pcm, &mut rendered));
    if rendered.len() > capacity {
        return 0;
    }
    // SAFETY: `capacity` writable floats.
    unsafe { std::slice::from_raw_parts_mut(out, rendered.len()).copy_from_slice(&rendered) };
    rendered.len()
}

/// What the pointer looks like: its picture's size and hot spot in pixels; all zero for the arrow.
#[no_mangle]
pub extern "C" fn sp_room_set_cursor_shape(room: *mut RoomHandle, hot_x: f64, hot_y: f64, width: f64, height: f64) {
    with_room(room, (), |r| r.set_cursor_shape(hot_x, hot_y, width, height));
}

#[no_mangle]
pub extern "C" fn sp_room_show(room: *mut RoomHandle, id: u32) {
    with_room(room, (), |r| r.show(id));
}

#[no_mangle]
pub extern "C" fn sp_room_remove(room: *mut RoomHandle, id: u32) {
    with_room(room, (), |r| r.remove_window(id));
}

#[no_mangle]
pub extern "C" fn sp_room_clear(room: *mut RoomHandle) {
    with_room(room, (), |r| r.clear());
}

#[no_mangle]
pub extern "C" fn sp_room_focus(room: *mut RoomHandle, id: i32) {
    with_room(room, (), |r| r.set_focus((id >= 0).then_some(id as u32)));
}

#[no_mangle]
pub extern "C" fn sp_room_focused(room: *mut RoomHandle) -> i32 {
    with_room(room, -1, |r| r.focus().map_or(-1, |i| i as i32))
}

#[no_mangle]
pub extern "C" fn sp_room_move_pointer(room: *mut RoomHandle, dx: f64, dy: f64) {
    with_room(room, (), |r| r.move_pointer(dx, dy));
}

#[no_mangle]
pub extern "C" fn sp_room_centre_pointer(room: *mut RoomHandle) {
    with_room(room, (), |r| r.centre_pointer());
}

#[no_mangle]
pub extern "C" fn sp_room_aim(room: *mut RoomHandle, out: *mut SpAim) {
    let aim = with_room(room, None, |r| Some(r.aim()));
    // SAFETY: a writable `SpAim`, by the contract.
    if let (Some(aim), Some(out)) = (aim, unsafe { out.as_mut() }) {
        out.window = aim.window.map_or(-1, |w| w as i32);
        out.x = aim.x;
        out.y = aim.y;
        out.point = aim.point.to_array();
        out.zone = aim.zone.map_or(0, |z| z.code());
    }
}

/// Where the cursor is in one window's pixels, off its edge as well as on it. 1 if there is such a window.
#[no_mangle]
pub extern "C" fn sp_room_aim_free(room: *mut RoomHandle, id: u32, x: *mut f64, y: *mut f64) -> i32 {
    match with_room(room, None, |r| r.aim_free(id)) {
        Some((ax, ay)) => {
            // SAFETY: writable doubles, by the contract.
            unsafe {
                if let Some(x) = x.as_mut() {
                    *x = ax;
                }
                if let Some(y) = y.as_mut() {
                    *y = ay;
                }
            }
            1
        }
        None => 0,
    }
}

// MARK: the Deck's menus

fn event_json(event: &spatiand_shell::ShellEvent) -> String {
    use spatiand_shell::ShellEvent as E;
    let value = match event {
        E::Hud(action) => serde_json::json!({ "hud": format!("{action:?}") }),
        E::Launch(app) => serde_json::json!({ "launch_local": app.name }),
        E::ModeChanged(mode) => serde_json::json!({ "mode": format!("{mode:?}") }),
        E::ChooseEnvironment(choice) => serde_json::json!({ "environment": match choice {
            spatiand_shell::EnvironmentChoice::Blank => serde_json::json!("blank"),
            spatiand_shell::EnvironmentChoice::Studio => serde_json::json!("studio"),
            spatiand_shell::EnvironmentChoice::File(i) => serde_json::json!(i),
        }}),
        E::ListDirectory(dir) => serde_json::json!({ "list_directory": dir }),
        E::AddEnvironment(name) => serde_json::json!({ "add_environment": name }),
        E::FocusWindow(id) => serde_json::json!({ "focus": id }),
        E::Controller(_) => serde_json::json!({ "controller": true }),
        E::CloseWindow(id) => serde_json::json!({ "close": id }),
        E::HideWindow { id, hidden } => serde_json::json!({ "hide": { "id": id, "hidden": hidden } }),
        E::PinWindow { id, pinned } => serde_json::json!({ "pin": { "id": id, "pinned": pinned } }),
        E::LaunchRemote { host, app } => serde_json::json!({ "launch_remote": { "host": host, "app": app } }),
        E::PairHost(address) => serde_json::json!({ "pair": address }),
        E::ConfirmPairing => serde_json::json!({ "confirm_pairing": true }),
        E::CancelPairing => serde_json::json!({ "cancel_pairing": true }),
        E::ForgetHost(address) => serde_json::json!({ "forget": address }),
        E::Bluetooth(_) => serde_json::json!({ "bluetooth": true }),
    };
    value.to_string()
}

fn json_out(text: Option<String>) -> *mut c_char {
    match text.and_then(|t| CString::new(t).ok()) {
        Some(c) => c.into_raw(),
        None => std::ptr::null_mut(),
    }
}

/// Feed the menus an intent: 0 up, 1 down, 2 left, 3 right, 4 accept, 5 back, 6 settings, 7 launcher,
/// 8 windows, 9 close, 10 hide. Returns what the shell asks the app to do, as JSON, or null; free it with
/// `sp_free_string`.
#[no_mangle]
pub extern "C" fn sp_shell_intent(room: *mut RoomHandle, intent: i32) -> *mut c_char {
    use spatiand_shell::{Intent, NavDirection};
    let intent = match intent {
        0 => Intent::Navigate(NavDirection::Up),
        1 => Intent::Navigate(NavDirection::Down),
        2 => Intent::Navigate(NavDirection::Left),
        3 => Intent::Navigate(NavDirection::Right),
        4 => Intent::Accept,
        5 => Intent::Back,
        6 => Intent::ToggleHud,
        7 => Intent::ToggleLauncher,
        8 => Intent::ToggleSwitcher,
        9 => Intent::Close,
        10 => Intent::Hide,
        _ => return std::ptr::null_mut(),
    };
    json_out(with_room(room, None, |r| r.shell_intent(intent)).map(|e| event_json(&e)))
}

/// Whether a menu is covering the world, and whether it wants the keyboard's text.
#[no_mangle]
pub extern "C" fn sp_shell_open(room: *mut RoomHandle) -> i32 {
    with_room(room, 0, |r| r.ui.open() as i32 | ((r.ui.shell.wants_text() as i32) << 1))
}

/// Draw the menus again if they changed, and hang them in the room. Returns a number that changes when
/// their pictures did. Slow (it sets text), so it works on the menus off the room's lock.
#[no_mangle]
pub extern "C" fn sp_shell_sync(room: *mut RoomHandle) -> u64 {
    let (mut ui, fov) = with_room(room, (shell_ui::ShellUi::new(), (40.0, 22.5)), |r| {
        let fov = (r.stereo().h_fov_deg, r.stereo().v_fov_deg());
        (std::mem::take(&mut r.ui), fov)
    });
    let changed = ui.render(fov);
    with_room(room, 0, move |r| {
        let version = ui.version;
        r.ui = ui;
        if changed {
            r.install_shell_panels();
        }
        version
    })
}

/// The ids of the panels the menus have, into `out`; how many there are.
#[no_mangle]
pub extern "C" fn sp_shell_panel_ids(room: *mut RoomHandle, out: *mut u32, capacity: usize) -> usize {
    let mut ids: Vec<u32> = with_room(room, Vec::new(), |r| r.ui.images.keys().copied().collect());
    ids.sort();
    ids.truncate(capacity);
    if !out.is_null() {
        // SAFETY: `capacity` writable integers, by the contract.
        unsafe { std::slice::from_raw_parts_mut(out, ids.len()).copy_from_slice(&ids) };
    }
    ids.len()
}

/// A menu panel's picture, premultiplied BGRA, into `out`. Returns whether it was there and fitted.
#[no_mangle]
pub extern "C" fn sp_shell_panel_image(room: *mut RoomHandle, id: u32, w: *mut u32, h: *mut u32, out: *mut u8, capacity: usize) -> i32 {
    let Some(image) = with_room(room, None, |r| r.ui.images.get(&id).map(|s| s.image.clone())) else { return 0 };
    // SAFETY: writable integers and `capacity` writable bytes, by the contract.
    unsafe {
        if let (Some(w), Some(h)) = (w.as_mut(), h.as_mut()) {
            *w = image.width;
            *h = image.height;
        }
        if out.is_null() || image.rgba.len() > capacity {
            return 0;
        }
        std::slice::from_raw_parts_mut(out, image.rgba.len()).copy_from_slice(&image.rgba);
    }
    1
}

/// The pointer is moving over an open menu: the shell's cursor goes to the row or bubble it is on.
#[no_mangle]
pub extern "C" fn sp_shell_hover(room: *mut RoomHandle) {
    with_room(room, (), |r| {
        if let Some(shell_ui::Target::Row(index)) = r.shell_target() {
            r.ui.point(index);
        }
    });
}

/// A press on an open menu. Returns what the shell asks the app to do, or null.
#[no_mangle]
pub extern "C" fn sp_shell_click(room: *mut RoomHandle) -> *mut c_char {
    json_out(with_room(room, None, |r| match r.shell_target() {
        Some(shell_ui::Target::Row(index)) => {
            r.ui.point(index);
            r.shell_intent(spatiand_shell::Intent::Accept)
        }
        Some(shell_ui::Target::Back) => r.shell_intent(spatiand_shell::Intent::Back),
        None => None,
    })
    .map(|e| event_json(&e)))
}

/// The computers: `{"rows": [{label, address, status}], "tabs": [{label, address, online, apps: [{id, name}]}]}`.
#[no_mangle]
pub extern "C" fn sp_shell_set_hosts(room: *mut RoomHandle, json: *const c_char) {
    if json.is_null() {
        return;
    }
    // SAFETY: a NUL-terminated string, by the contract.
    let text = unsafe { CStr::from_ptr(json) }.to_string_lossy().into_owned();
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else { return };
    let text_of = |v: &serde_json::Value, k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or_default().to_string();
    let rows = value["rows"].as_array().cloned().unwrap_or_default().iter().map(|r| spatiand_shell::HostRow {
        label: text_of(r, "label"),
        address: text_of(r, "address"),
        status: match r.get("status").and_then(|x| x.as_str()) {
            Some("online") => spatiand_shell::HostStatus::Online,
            Some("connecting") => spatiand_shell::HostStatus::Connecting,
            Some("refused") => spatiand_shell::HostStatus::Refused,
            _ => spatiand_shell::HostStatus::Offline,
        },
    }).collect();
    let tabs = value["tabs"].as_array().cloned().unwrap_or_default().iter().map(|t| spatiand_shell::HostTab {
        label: text_of(t, "label"),
        address: text_of(t, "address"),
        online: t.get("online").and_then(|x| x.as_bool()).unwrap_or(false),
        apps: t["apps"].as_array().cloned().unwrap_or_default().iter().map(|a| spatiand_shell::RemoteEntry {
            id: text_of(a, "id"),
            name: text_of(a, "name"),
            icon: None,
        }).collect(),
    }).collect();
    with_room(room, (), |r| r.ui.set_hosts(rows, tabs));
}

/// The menus are out of date and should be drawn again.
#[no_mangle]
pub extern "C" fn sp_shell_dirty(room: *mut RoomHandle) {
    with_room(room, (), |r| r.ui.dirty = true);
}

/// Where pinned windows sit (0 to 3, bottom right first) and whether they are large, for the settings rows.
#[no_mangle]
pub extern "C" fn sp_shell_set_pip(room: *mut RoomHandle, corner: i32, large: i32) {
    with_room(room, (), |r| {
        r.ui.shell.set_pip(corner.clamp(0, 3) as usize, large != 0);
        r.ui.dirty = true;
    });
}

/// A launcher bubble's picture, by the application's name: straight RGBA.
#[no_mangle]
pub extern "C" fn sp_shell_set_icon(room: *mut RoomHandle, name: *const c_char, width: u32, height: u32, rgba: *const u8) {
    if name.is_null() || rgba.is_null() || width == 0 || height == 0 || width > 512 || height > 512 {
        return;
    }
    // SAFETY: a NUL-terminated string and `width * height * 4` readable bytes, by the contract.
    let (name, pixels) = unsafe { (CStr::from_ptr(name).to_string_lossy().into_owned(), std::slice::from_raw_parts(rgba, (width * height * 4) as usize).to_vec()) };
    with_room(room, (), |r| r.ui.set_icon(&name, width, height, pixels));
}

/// The environments on offer: whether the studio is the one in use. (Blank and the studio are the two the
/// Mac has.)
#[no_mangle]
pub extern "C" fn sp_shell_set_studio(room: *mut RoomHandle, studio: i32) {
    use spatiand_shell::EnvironmentChoice as C;
    let choices = vec![("Studio".to_string(), C::Studio), ("Blank".to_string(), C::Blank)];
    with_room(room, (), |r| r.ui.set_environments(choices, if studio != 0 { C::Studio } else { C::Blank }));
}

/// Text typed while the shell asks for it (a computer's address): returns an event JSON when Return is hit.
#[no_mangle]
pub extern "C" fn sp_shell_type(room: *mut RoomHandle, text: *const c_char, backspace: i32, enter: i32) -> *mut c_char {
    let text = if text.is_null() { String::new() } else { unsafe { CStr::from_ptr(text) }.to_string_lossy().into_owned() };
    json_out(with_room(room, None, |r| {
        r.ui.dirty = true;
        if !text.is_empty() {
            r.ui.shell.type_text(&text);
        }
        if backspace != 0 {
            r.ui.shell.type_backspace();
        }
        if enter != 0 {
            return r.ui.shell.type_enter();
        }
        None
    })
    .map(|e| event_json(&e)))
}

#[no_mangle]
pub extern "C" fn sp_room_set_title(room: *mut RoomHandle, id: u32, title: *const c_char) {
    if title.is_null() {
        return;
    }
    // SAFETY: a NUL-terminated string, by the contract.
    let title = unsafe { CStr::from_ptr(title) }.to_string_lossy().into_owned();
    with_room(room, (), |r| r.set_title(id, &title));
}

/// The application's icon: `width * height * 4` bytes of straight RGBA.
#[no_mangle]
pub extern "C" fn sp_room_set_icon(room: *mut RoomHandle, id: u32, width: u32, height: u32, rgba: *const u8) {
    if rgba.is_null() || width == 0 || height == 0 || width > 512 || height > 512 {
        return;
    }
    // SAFETY: `width * height * 4` readable bytes, by the contract.
    let pixels = unsafe { std::slice::from_raw_parts(rgba, (width * height * 4) as usize) }.to_vec();
    with_room(room, (), |r| r.set_icon(id, width, height, pixels));
}

#[no_mangle]
pub extern "C" fn sp_room_set_sound(room: *mut RoomHandle, id: u32, sounding: i32, muted: i32) {
    with_room(room, (), |r| r.set_sound(id, sounding != 0, muted != 0));
}

/// What the pointer is over on a window's chrome (a zone code; 0 for nothing).
#[no_mangle]
pub extern "C" fn sp_room_set_hover(room: *mut RoomHandle, id: u32, zone: i32) {
    with_room(room, (), |r| r.set_hover(chrome::Zone::from_code(zone).map(|z| (id, z))));
}

/// Take a window by one of its edges (a zone code, 7 to 11).
#[no_mangle]
pub extern "C" fn sp_room_begin_resize(room: *mut RoomHandle, id: u32, zone: i32) {
    if let Some(chrome::Zone::Resize(edge)) = chrome::Zone::from_code(zone) {
        with_room(room, (), |r| r.begin_resize(id, edge));
    }
}

/// Carry the edge along with the pointer. 1, and the size to ask the application for, if a resize is
/// going on.
#[no_mangle]
pub extern "C" fn sp_room_drag_resize(room: *mut RoomHandle, id: *mut u32, w: *mut u32, h: *mut u32) -> i32 {
    match with_room(room, None, |r| r.drag_resize()) {
        Some((window, (pw, ph))) => {
            // SAFETY: writable integers, by the contract.
            unsafe {
                if let (Some(id), Some(w), Some(h)) = (id.as_mut(), w.as_mut(), h.as_mut()) {
                    *id = window;
                    *w = pw;
                    *h = ph;
                }
            }
            1
        }
        None => 0,
    }
}

#[no_mangle]
pub extern "C" fn sp_room_end_resize(room: *mut RoomHandle) {
    with_room(room, (), |r| r.end_resize());
}

#[no_mangle]
pub extern "C" fn sp_room_is_sizing(room: *mut RoomHandle) -> i32 {
    with_room(room, -1, |r| r.is_sizing().map_or(-1, |i| i as i32))
}

/// Put a window nearer (negative) or further (positive), by this many metres.
#[no_mangle]
pub extern "C" fn sp_room_push_pull(room: *mut RoomHandle, id: u32, metres: f64) {
    with_room(room, (), |r| r.push_pull(id, metres));
}

#[no_mangle]
pub extern "C" fn sp_room_set_hidden(room: *mut RoomHandle, id: u32, hidden: i32) {
    with_room(room, (), |r| r.set_hidden(id, hidden != 0));
}

#[no_mangle]
pub extern "C" fn sp_room_is_hidden(room: *mut RoomHandle, id: u32) -> i32 {
    with_room(room, 0, |r| r.is_hidden(id) as i32)
}

/// A number that changes when a window's chrome has to be drawn again; 0 for a window without any.
#[no_mangle]
pub extern "C" fn sp_room_chrome_version(room: *mut RoomHandle, id: u32) -> u64 {
    with_room(room, None, |r| r.look_of(id)).map_or(0, |l| chrome::look_key(&l))
}

/// A window's chrome as premultiplied BGRA, `width * height * 4` bytes, into `out`; the size is
/// written to `w` and `h` first, and nothing is drawn if it would not fit in `capacity`. Returns whether
/// it was drawn. Made off the room's lock: it sets text, which takes a moment.
#[no_mangle]
pub extern "C" fn sp_room_chrome_render(room: *mut RoomHandle, id: u32, width_px: u32, w: *mut u32, h: *mut u32, out: *mut u8, capacity: usize) -> i32 {
    let Some(look) = with_room(room, None, |r| r.look_of(id)) else { return 0 };
    let (cw, ch, mut rgba) = chrome::compose(&look, width_px);
    // SAFETY: writable integers and `capacity` writable bytes, by the contract.
    unsafe {
        if let (Some(w), Some(h)) = (w.as_mut(), h.as_mut()) {
            *w = cw;
            *h = ch;
        }
        if out.is_null() || rgba.len() > capacity {
            return 0;
        }
    }
    // Premultiplied, and in the order Metal's BGRA wants.
    for px in rgba.chunks_exact_mut(4) {
        let a = px[3] as u32;
        let (r, g, b) = (px[0] as u32 * a / 255, px[1] as u32 * a / 255, px[2] as u32 * a / 255);
        px[0] = b as u8;
        px[1] = g as u8;
        px[2] = r as u8;
    }
    // SAFETY: as above.
    unsafe { std::slice::from_raw_parts_mut(out, rgba.len()).copy_from_slice(&rgba) };
    1
}

/// The Deck's pointer: a dot in a ring. White, straight RGBA, `size * size * 4` bytes.
#[no_mangle]
pub extern "C" fn sp_reticle(size: u32, out: *mut u8) {
    if out.is_null() || size == 0 || size > 512 {
        return;
    }
    let image = look::reticle_image(size);
    // SAFETY: `size * size * 4` writable bytes, by the contract.
    unsafe { std::slice::from_raw_parts_mut(out, image.len()).copy_from_slice(&image) };
}

/// Start finding the machine's fonts now, so a window's title is ready when it is first drawn.
#[no_mangle]
pub extern "C" fn sp_text_warm_up() {
    chrome::warm_up();
}

/// Where the cursor is in one window's pixels, even off its edge. 1 if there is such a window.
#[no_mangle]
pub extern "C" fn sp_room_aim_at(room: *mut RoomHandle, id: u32, x: *mut f64, y: *mut f64) -> i32 {
    match with_room(room, None, |r| r.aim_at(id)) {
        Some((ax, ay)) => {
            // SAFETY: writable doubles, by the contract.
            unsafe {
                if let Some(x) = x.as_mut() {
                    *x = ax;
                }
                if let Some(y) = y.as_mut() {
                    *y = ay;
                }
            }
            1
        }
        None => 0,
    }
}

#[no_mangle]
pub extern "C" fn sp_room_begin_grab(room: *mut RoomHandle, id: u32) {
    with_room(room, (), |r| r.begin_grab(id));
}

#[no_mangle]
pub extern "C" fn sp_room_drag(room: *mut RoomHandle) {
    with_room(room, (), |r| r.drag());
}

#[no_mangle]
pub extern "C" fn sp_room_end_grab(room: *mut RoomHandle) {
    with_room(room, (), |r| r.end_grab());
}

#[no_mangle]
pub extern "C" fn sp_room_grabbed(room: *mut RoomHandle) -> i32 {
    with_room(room, -1, |r| r.grabbed().map_or(-1, |i| i as i32))
}

#[no_mangle]
pub extern "C" fn sp_room_nudge(room: *mut RoomHandle, id: u32, yaw_deg: f64, pitch_deg: f64) {
    with_room(room, (), |r| r.nudge(id, yaw_deg.to_radians(), pitch_deg.to_radians()));
}

/// Move the keyboard to the next window round the room: +1 to the left, -1 to the right.
/// Returns its id, or -1 if there is none.
#[no_mangle]
pub extern "C" fn sp_room_focus_step(room: *mut RoomHandle, step: i32) -> i32 {
    with_room(room, -1, |r| r.focus_step(step).map_or(-1, |i| i as i32))
}

/// Gather the windows side by side in front of the wearer; `spread` is the room between them.
#[no_mangle]
pub extern "C" fn sp_room_arrange(room: *mut RoomHandle, spread: f64) {
    with_room(room, (), |r| r.arrange(spread));
}

#[no_mangle]
pub extern "C" fn sp_room_scale(room: *mut RoomHandle, id: u32, factor: f64) {
    with_room(room, (), |r| r.scale(id, factor));
}

#[no_mangle]
pub extern "C" fn sp_room_bring_here(room: *mut RoomHandle, id: u32) {
    with_room(room, (), |r| r.bring_here(id));
}

#[no_mangle]
pub extern "C" fn sp_room_set_pinned(room: *mut RoomHandle, id: u32, pinned: i32) {
    with_room(room, (), |r| r.set_pinned(id, pinned != 0));
}

#[no_mangle]
pub extern "C" fn sp_room_is_pinned(room: *mut RoomHandle, id: u32) -> i32 {
    with_room(room, 0, |r| r.is_pinned(id) as i32)
}

#[no_mangle]
pub extern "C" fn sp_room_next_corner(room: *mut RoomHandle) {
    with_room(room, (), |r| r.next_corner());
}

#[no_mangle]
pub extern "C" fn sp_room_toggle_size(room: *mut RoomHandle) {
    with_room(room, (), |r| r.toggle_size());
}

/// What to draw. `matrices` takes 32 floats: the left eye's view-projection then the right's,
/// column-major. Vertices are `x y z u v`; the number of floats written is returned (0 if they
/// would not fit), and `draw_count` says how many of `draws` are filled.
#[no_mangle]
pub extern "C" fn sp_room_frame(
    room: *mut RoomHandle,
    matrices: *mut f32,
    vertices: *mut f32,
    vertex_capacity: u32,
    draws: *mut SpDraw,
    draw_capacity: u32,
    draw_count: *mut u32,
) -> u32 {
    let Some(frame) = with_room(room, None, |r| Some(r.frame())) else { return 0 };
    if frame.vertices.len() > vertex_capacity as usize || frame.draws.len() > draw_capacity as usize {
        return 0;
    }
    // SAFETY: buffers of the sizes given, by the contract.
    unsafe {
        if !matrices.is_null() {
            let out = std::slice::from_raw_parts_mut(matrices, 32);
            out[..16].copy_from_slice(&frame.eyes[0].to_cols_array());
            out[16..].copy_from_slice(&frame.eyes[1].to_cols_array());
        }
        if !vertices.is_null() {
            std::slice::from_raw_parts_mut(vertices, frame.vertices.len()).copy_from_slice(&frame.vertices);
        }
        if !draws.is_null() {
            let out = std::slice::from_raw_parts_mut(draws, frame.draws.len());
            for (slot, d) in out.iter_mut().zip(&frame.draws) {
                *slot = SpDraw {
                    window: d.window,
                    first: d.first,
                    count: d.count,
                    flags: d.focused as u32 | (d.aimed as u32) << 1 | (d.pinned as u32) << 2,
                };
            }
        }
        if let Some(n) = draw_count.as_mut() {
            *n = frame.draws.len() as u32;
        }
    }
    frame.vertices.len() as u32
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
