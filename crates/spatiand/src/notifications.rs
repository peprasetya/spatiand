//! Desktop applications' notifications, shown on the room's card on the Deck.
//!
//! An application on Linux says something to the wearer by calling
//! `org.freedesktop.Notifications.Notify` on the session bus. On the desktop Plasma answers that
//! name; in the room nothing did, and a download finishing or a message arriving went nowhere. So
//! the room answers it, and shows what it is told on the same card under the status bar the Mac
//! shows its banners on: who it is from on the first line, what it says on the second. Pressing
//! the card is the notification's default action (an application usually brings its window
//! forward), and the card goes.
//!
//! **Plasma is not fought for the name.** It is asked for with `ALLOW_REPLACEMENT` and
//! `DO_NOT_QUEUE`: if someone has it, it is left to them and asked for again every ten seconds,
//! and if someone takes it away (`NameLost`) the card stops showing and the asking begins again.
//! The room is its own session on the Deck, where Plasma's shell does not run, so in practice the
//! name is free; a room started anywhere else leaves the desktop's server be.
//!
//! **The D-Bus here is written out by hand**, as `bluetooth.rs` keeps BlueZ's to a command-line
//! client: the compositor carries no D-Bus stack for one card. What is needed is small -- the
//! `EXTERNAL` handshake, `Hello`, `RequestName`, the five methods of the specification and its
//! two signals -- and the one part with any depth, the hints a notification carries (an image is
//! a `(iiibiiay)` in a variant), is skipped by its signature rather than understood.
//!
//! The bus is read on a thread of its own. Replies and signals are written from whichever thread
//! has something to say -- the frame loop's press, or the reader's own -- through one shared
//! handle on the socket.

use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::notices::{words as card, Card};

const NAME: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";
const BUS: &str = "org.freedesktop.DBus";
const BUS_PATH: &str = "/org/freedesktop/DBus";
/// How long the name is left to whoever has it before it is asked for again.
const ASK_AGAIN: Duration = Duration::from_secs(10);
/// A notification that leaves it to the server (`expire_timeout` -1).
const DEFAULT_FOR: Duration = Duration::from_secs(10);
/// One that asks never to go (0) goes when pressed or closed, or after this: a card left over a
/// room the wearer has walked away from is not worth keeping for ever.
const NEVER_FOR: Duration = Duration::from_secs(600);
/// What an application's own time is held between: shorter cannot be read, longer is in the way.
const SHORTEST: Duration = Duration::from_secs(3);
const LONGEST: Duration = Duration::from_secs(60);

// Message types and flags (the D-Bus specification's).
const CALL: u8 = 1;
const RETURN: u8 = 2;
const ERROR: u8 = 3;
const SIGNAL: u8 = 4;
const NO_REPLY_EXPECTED: u8 = 1;
// RequestName's flags.
const ALLOW_REPLACEMENT: u32 = 1;
const DO_NOT_QUEUE: u32 = 4;

/// The card, by notification id: the room's one card logic (`crate::notices`), shared with the
/// Beam Pro's and the Mac's.
struct State {
    card: Card,
    /// The notification on the card, if its actions include "default", which a press is.
    default_action: Option<u32>,
    /// The last id handed out; they start at 1, as 0 means "replaces nothing".
    last_id: u32,
}

static STATE: Mutex<State> = Mutex::new(State { card: Card::new(), default_action: None, last_id: 0 });

fn id_of(key: &str) -> u32 {
    key.parse().unwrap_or(0)
}
/// The socket's writing side, shared by every thread that answers or signals.
static OUT: Mutex<Option<UnixStream>> = Mutex::new(None);
static SERIAL: AtomicU32 = AtomicU32::new(1);

// --- what the room asks ---

/// Start serving on the session bus, on a thread of its own. Returns at once. Takes the name with
/// `ALLOW_REPLACEMENT | DO_NOT_QUEUE`: if Plasma (or anyone) has it, it is left to them and asked
/// again every ten seconds; on `NameLost` the card stops showing and the asking starts again. No
/// session bus: said once in the log, and nothing more is tried.
pub fn start() {
    if let Err(e) = std::thread::Builder::new().name("notifications".into()).spawn(serve) {
        log::warn!("notifications: could not start its thread: {e}");
    }
}

/// The card's text: who it is from on the first line, what it says on the second; empty when
/// nothing is on show. One that has had its time is closed here.
pub fn notice() -> String {
    expire();
    STATE.lock().unwrap().card.text(Instant::now())
}

/// Whether a notification is on show now, so that a press on the card is this module's.
pub fn showing() -> bool {
    !notice().is_empty()
}

/// The card was pressed: the notification's default action, if it has one, and it is closed as
/// dismissed by the wearer.
pub fn pressed() {
    let taken = {
        let mut state = STATE.lock().unwrap();
        let id = state.card.press(Instant::now()).map(|k| id_of(&k));
        id.map(|id| (id, state.default_action.take() == Some(id)))
    };
    if let Some((id, default_action)) = taken {
        log::info!("notification {id} pressed");
        if default_action {
            invoked(id, "default");
        }
        closed(id, 2);
    }
}

fn expire() {
    let gone = STATE.lock().unwrap().card.expire(Instant::now()).map(|k| id_of(&k));
    if let Some(id) = gone {
        closed(id, 1);
    }
}

// --- the specification's two signals ---

/// `NotificationClosed`: 1 it expired, 2 the wearer dismissed it, 3 `CloseNotification` closed it.
fn closed(id: u32, reason: u32) {
    let mut w = W::default();
    w.u(id);
    w.u(reason);
    signal("NotificationClosed", "uu", &w.b);
}

fn invoked(id: u32, key: &str) {
    let mut w = W::default();
    w.u(id);
    w.s(key);
    signal("ActionInvoked", "us", &w.b);
}

fn signal(member: &str, signature: &str, body: &[u8]) {
    let header = Header { path: Some(PATH), interface: Some(NAME), member: Some(member), ..Header::default() };
    send(&encode(SIGNAL, 0, next_serial(), &header, signature, body));
}

fn next_serial() -> u32 {
    // 0 is not a serial.
    match SERIAL.fetch_add(1, Ordering::Relaxed) {
        0 => SERIAL.fetch_add(1, Ordering::Relaxed),
        n => n,
    }
}

/// Write a whole message. Nothing to write to (no bus, or it went away) is not an error: the
/// card is the room's either way.
fn send(message: &[u8]) -> bool {
    match OUT.lock().unwrap().as_mut() {
        Some(stream) => stream.write_all(message).is_ok(),
        None => false,
    }
}

// --- the bus ---

fn serve() {
    let mut connected_once = false;
    loop {
        match Conn::open() {
            Ok(conn) => {
                connected_once = true;
                if let Err(e) = session(conn) {
                    log::warn!("notifications: the session bus went away ({e})");
                }
                *OUT.lock().unwrap() = None;
                STATE.lock().unwrap().card.clear();
            }
            Err(e) if !connected_once => {
                log::info!("notifications: no session bus ({e}); desktop applications' notifications are not shown in the room");
                return;
            }
            Err(_) => {}
        }
        std::thread::sleep(ASK_AGAIN);
    }
}

/// One connection, from `Hello` until the bus goes.
fn session(mut conn: Conn) -> io::Result<()> {
    conn.stream.set_read_timeout(Some(Duration::from_secs(1)))?;
    *OUT.lock().unwrap() = Some(conn.stream.try_clone()?);
    let hello = call(BUS, BUS_PATH, BUS, "Hello", "", &[])?;
    let asked = Instant::now();
    loop {
        if asked.elapsed() > Duration::from_secs(5) {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "the bus did not answer Hello"));
        }
        if let Some(m) = conn.next()? {
            if m.reply_serial == Some(hello) {
                if m.kind == ERROR {
                    return Err(io::Error::other("the bus refused Hello"));
                }
                break;
            }
        }
    }
    let mut owned = false;
    // What was last said in the log about the name, so that asking every ten seconds does not
    // say it every ten seconds.
    let mut said: Option<bool> = None;
    let mut asking: Option<u32> = None;
    let mut next_ask = Instant::now();
    loop {
        if !owned && asking.is_none() && Instant::now() >= next_ask {
            let mut w = W::default();
            w.s(NAME);
            w.u(ALLOW_REPLACEMENT | DO_NOT_QUEUE);
            asking = Some(call(BUS, BUS_PATH, BUS, "RequestName", "su", &w.b)?);
            next_ask = Instant::now() + ASK_AGAIN;
        }
        expire();
        let Some(m) = conn.next()? else { continue };
        match m.kind {
            RETURN | ERROR if m.reply_serial.is_some() && m.reply_serial == asking => {
                asking = None;
                // 1: it is ours now; 4: it already was. 3: someone has it.
                let answer = if m.kind == RETURN { R::new(&m.body, m.big).u32().ok() } else { None };
                if let Some(error) = &m.error {
                    log::warn!("notifications: asking for {NAME}: {error}");
                }
                owned = matches!(answer, Some(1) | Some(4));
            }
            SIGNAL if m.interface.as_deref() == Some(BUS) => {
                let about = R::new(&m.body, m.big).s().ok();
                if about.as_deref() == Some(NAME) {
                    match m.member.as_deref() {
                        Some("NameAcquired") => owned = true,
                        Some("NameLost") => {
                            owned = false;
                            STATE.lock().unwrap().card.clear();
                            next_ask = Instant::now() + ASK_AGAIN;
                        }
                        _ => {}
                    }
                }
            }
            CALL => answer(&m),
            _ => {}
        }
        if said != Some(owned) && (asking.is_none() || owned) {
            if owned {
                log::info!("notifications: serving {NAME}; desktop applications' notifications are shown on the card");
            } else {
                log::info!("notifications: {NAME} is someone else's; left to them, asked again every {}s", ASK_AGAIN.as_secs());
            }
            said = Some(owned);
        }
    }
}

fn call(destination: &str, path: &str, interface: &str, member: &str, signature: &str, body: &[u8]) -> io::Result<u32> {
    let serial = next_serial();
    let header = Header {
        path: Some(path),
        interface: Some(interface),
        member: Some(member),
        destination: Some(destination),
        ..Header::default()
    };
    if send(&encode(CALL, 0, serial, &header, signature, body)) {
        Ok(serial)
    } else {
        Err(io::Error::new(io::ErrorKind::BrokenPipe, "could not write to the bus"))
    }
}

const INTROSPECTION: &str = r#"<!DOCTYPE node PUBLIC "-//freedesktop//DTD D-BUS Object Introspection 1.0//EN" "http://www.freedesktop.org/standards/dbus/1.0/introspect.dtd">
<node>
  <interface name="org.freedesktop.Notifications">
    <method name="GetCapabilities"><arg direction="out" type="as"/></method>
    <method name="Notify">
      <arg name="app_name" direction="in" type="s"/>
      <arg name="replaces_id" direction="in" type="u"/>
      <arg name="app_icon" direction="in" type="s"/>
      <arg name="summary" direction="in" type="s"/>
      <arg name="body" direction="in" type="s"/>
      <arg name="actions" direction="in" type="as"/>
      <arg name="hints" direction="in" type="a{sv}"/>
      <arg name="expire_timeout" direction="in" type="i"/>
      <arg name="id" direction="out" type="u"/>
    </method>
    <method name="CloseNotification"><arg name="id" direction="in" type="u"/></method>
    <method name="GetServerInformation">
      <arg name="name" direction="out" type="s"/>
      <arg name="vendor" direction="out" type="s"/>
      <arg name="version" direction="out" type="s"/>
      <arg name="spec_version" direction="out" type="s"/>
    </method>
    <signal name="NotificationClosed"><arg name="id" type="u"/><arg name="reason" type="u"/></signal>
    <signal name="ActionInvoked"><arg name="id" type="u"/><arg name="action_key" type="s"/></signal>
  </interface>
  <interface name="org.freedesktop.DBus.Introspectable">
    <method name="Introspect"><arg name="xml" direction="out" type="s"/></method>
  </interface>
  <interface name="org.freedesktop.DBus.Peer">
    <method name="Ping"/>
    <method name="GetMachineId"><arg name="machine_uuid" direction="out" type="s"/></method>
  </interface>
</node>
"#;

/// Answer a method call, unless it asked for no answer.
fn answer(m: &Msg) {
    let interface = m.interface.as_deref();
    let member = m.member.as_deref().unwrap_or("");
    let mut w = W::default();
    let reply: Result<&str, (&str, String)> = match (interface, member) {
        (Some(NAME) | None, "Notify") => notify(m).map(|id| {
            w.u(id);
            "u"
        }),
        (Some(NAME) | None, "GetCapabilities") => {
            // Only "body": no markup, no buttons, no icons -- the card is two lines of text.
            w.array(4, |w| w.s("body"));
            Ok("as")
        }
        (Some(NAME) | None, "GetServerInformation") => {
            for part in ["Spatiand", "Spatiand", env!("CARGO_PKG_VERSION"), "1.2"] {
                w.s(part);
            }
            Ok("ssss")
        }
        (Some(NAME) | None, "CloseNotification") => close(m).map(|()| ""),
        (Some("org.freedesktop.DBus.Introspectable") | None, "Introspect") => {
            w.s(INTROSPECTION);
            Ok("s")
        }
        (Some("org.freedesktop.DBus.Peer") | None, "Ping") => Ok(""),
        (Some("org.freedesktop.DBus.Peer") | None, "GetMachineId") => {
            let id = ["/etc/machine-id", "/var/lib/dbus/machine-id"]
                .iter()
                .find_map(|p| std::fs::read_to_string(p).ok())
                .map(|s| s.trim().to_string())
                .unwrap_or_default();
            w.s(&id);
            Ok("s")
        }
        _ => Err((
            "org.freedesktop.DBus.Error.UnknownMethod",
            format!("{} has no {}.{member}", m.path.as_deref().unwrap_or(PATH), interface.unwrap_or("?")),
        )),
    };
    if m.flags & NO_REPLY_EXPECTED != 0 {
        return;
    }
    let mut header = Header { reply_serial: Some(m.serial), destination: m.sender.as_deref(), ..Header::default() };
    let message = match reply {
        Ok(signature) => encode(RETURN, NO_REPLY_EXPECTED, next_serial(), &header, signature, &w.b),
        Err((name, text)) => {
            header.error = Some(name);
            let mut w = W::default();
            w.s(&text);
            encode(ERROR, NO_REPLY_EXPECTED, next_serial(), &header, "s", &w.b)
        }
    };
    send(&message);
}

/// `Notify`: put it on the card, in place of whatever was there.
fn notify(m: &Msg) -> Result<u32, (&'static str, String)> {
    let invalid = |e: String| ("org.freedesktop.DBus.Error.InvalidArgs", e);
    if m.signature != "susssasa{sv}i" {
        return Err(invalid(format!("Notify takes susssasa{{sv}}i, not {}", m.signature)));
    }
    let n = read_notify(&m.body, m.big).map_err(invalid)?;
    let text = card(&n.app, &n.summary, &n.body);
    let default_action = n.actions.chunks(2).any(|pair| pair[0] == "default");
    let (id, posted) = {
        let mut state = STATE.lock().unwrap();
        let id = if n.replaces != 0 {
            state.last_id = state.last_id.max(n.replaces);
            n.replaces
        } else {
            state.last_id = state.last_id.checked_add(1).unwrap_or(1);
            state.last_id
        };
        let posted = state.card.post(&id.to_string(), text, lasting(n.timeout), Instant::now());
        if posted.shown {
            state.default_action = default_action.then_some(id);
        }
        (id, posted)
    };
    log::info!("notification {id} from {:?}", n.app);
    // The one it pushed off the card is over, as far as its application need know.
    if let Some(old) = posted.displaced {
        closed(id_of(&old), 1);
    }
    if !posted.shown {
        // Nothing to show: over as soon as it began.
        closed(id, 1);
    }
    Ok(id)
}

fn close(m: &Msg) -> Result<(), (&'static str, String)> {
    let id = R::new(&m.body, m.big).u32().map_err(|e| ("org.freedesktop.DBus.Error.InvalidArgs", e))?;
    if STATE.lock().unwrap().card.remove(&id.to_string()) {
        closed(id, 3);
    }
    Ok(())
}

/// `Notify`'s arguments, as far as the card needs them.
#[derive(Debug, PartialEq)]
struct Notify {
    app: String,
    replaces: u32,
    summary: String,
    body: String,
    actions: Vec<String>,
    timeout: i32,
}

fn read_notify(body: &[u8], big: bool) -> Result<Notify, String> {
    let mut r = R::new(body, big);
    let app = r.s()?;
    let replaces = r.u32()?;
    let _icon = r.s()?;
    let summary = r.s()?;
    let text = r.s()?;
    let length = r.u32()? as usize;
    r.align(4)?;
    let end = r.p + length;
    let mut actions = Vec::new();
    while r.p < end {
        actions.push(r.s()?);
    }
    if r.p != end {
        return Err("the actions run past their array".into());
    }
    // The hints: an urgency, a category, perhaps a picture. None of them changes a card of text.
    r.skip(b"a{sv}")?;
    let timeout = r.i32()?;
    Ok(Notify { app, replaces, summary, body: text, actions, timeout })
}

/// How long a notification stays, from what it asked for.
fn lasting(expire_ms: i32) -> Duration {
    match expire_ms {
        i32::MIN..=-1 => DEFAULT_FOR,
        0 => NEVER_FOR,
        ms => Duration::from_millis(ms as u64).clamp(SHORTEST, LONGEST),
    }
}

// --- the wire ---

/// A message, as far as this needs one.
#[derive(Debug, Default)]
struct Msg {
    kind: u8,
    flags: u8,
    serial: u32,
    path: Option<String>,
    interface: Option<String>,
    member: Option<String>,
    error: Option<String>,
    reply_serial: Option<u32>,
    sender: Option<String>,
    signature: String,
    body: Vec<u8>,
    /// Whether its sender wrote it big-endian. Everything this writes is little-endian.
    big: bool,
}

#[derive(Default)]
struct Header<'a> {
    path: Option<&'a str>,
    interface: Option<&'a str>,
    member: Option<&'a str>,
    error: Option<&'a str>,
    reply_serial: Option<u32>,
    destination: Option<&'a str>,
}

fn encode(kind: u8, flags: u8, serial: u32, h: &Header, signature: &str, body: &[u8]) -> Vec<u8> {
    fn field(w: &mut W, code: u8, signature: &str, put: impl FnOnce(&mut W)) {
        w.pad(8);
        w.y(code);
        w.g(signature);
        put(w);
    }
    let mut w = W::default();
    w.y(b'l');
    w.y(kind);
    w.y(flags);
    w.y(1);
    w.u(body.len() as u32);
    w.u(serial);
    w.array(8, |w| {
        if let Some(v) = h.path {
            field(w, 1, "o", |w| w.s(v));
        }
        if let Some(v) = h.interface {
            field(w, 2, "s", |w| w.s(v));
        }
        if let Some(v) = h.member {
            field(w, 3, "s", |w| w.s(v));
        }
        if let Some(v) = h.error {
            field(w, 4, "s", |w| w.s(v));
        }
        if let Some(v) = h.reply_serial {
            field(w, 5, "u", |w| w.u(v));
        }
        if let Some(v) = h.destination {
            field(w, 6, "s", |w| w.s(v));
        }
        if !signature.is_empty() {
            field(w, 8, "g", |w| w.g(signature));
        }
    });
    w.pad(8);
    w.b.extend_from_slice(body);
    w.b
}

fn round8(n: usize) -> usize {
    n.div_ceil(8) * 8
}

/// How long the message at the start of `bytes` is, once its fixed part has arrived.
fn frame_len(bytes: &[u8]) -> io::Result<Option<usize>> {
    if bytes.len() < 16 {
        return Ok(None);
    }
    let big = match bytes[0] {
        b'l' => false,
        b'B' => true,
        _ => return Err(io::Error::new(io::ErrorKind::InvalidData, "not a D-Bus message")),
    };
    let word = |at: usize| {
        let b = [bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]];
        if big { u32::from_be_bytes(b) } else { u32::from_le_bytes(b) }
    };
    let total = round8(16 + word(12) as usize) + word(4) as usize;
    if total > 1 << 27 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "a message larger than the specification allows"));
    }
    Ok(Some(total))
}

fn parse(bytes: &[u8]) -> Result<Msg, String> {
    let big = bytes[0] == b'B';
    let mut r = R { b: bytes, p: 4, big };
    let body_len = r.u32()? as usize;
    let serial = r.u32()?;
    let fields_len = r.u32()? as usize;
    let end = 16 + fields_len;
    let mut m = Msg { kind: bytes[1], flags: bytes[2], serial, big, ..Msg::default() };
    let mut f = R { b: bytes.get(..end).ok_or("short header")?, p: 16, big };
    while f.p < end {
        f.align(8)?;
        let code = f.y()?;
        let signature = f.g()?;
        match (code, signature.as_str()) {
            (1, "o") => m.path = Some(f.s()?),
            (2, "s") => m.interface = Some(f.s()?),
            (3, "s") => m.member = Some(f.s()?),
            (4, "s") => m.error = Some(f.s()?),
            (5, "u") => m.reply_serial = Some(f.u32()?),
            (7, "s") => m.sender = Some(f.s()?),
            (8, "g") => m.signature = f.g()?,
            _ => f.skip(signature.as_bytes())?,
        }
    }
    let head = round8(end);
    m.body = bytes.get(head..head + body_len).ok_or("short body")?.to_vec();
    Ok(m)
}

enum Target {
    Path(PathBuf),
    Abstract(Vec<u8>),
}

/// Where the session bus may be: every `unix:` address in `DBUS_SESSION_BUS_ADDRESS`, in order,
/// or failing any, the usual socket in the runtime folder.
fn targets(address: Option<&str>, runtime: Option<&str>) -> Vec<Target> {
    use std::os::unix::ffi::OsStrExt;
    let mut found = Vec::new();
    for entry in address.unwrap_or_default().split(';') {
        let Some(rest) = entry.strip_prefix("unix:") else { continue };
        for pair in rest.split(',') {
            match pair.split_once('=') {
                Some(("path", v)) => found.push(Target::Path(std::ffi::OsStr::from_bytes(&unescape(v)).into())),
                Some(("abstract", v)) => found.push(Target::Abstract(unescape(v))),
                _ => {}
            }
        }
    }
    if found.is_empty() {
        if let Some(runtime) = runtime {
            found.push(Target::Path(PathBuf::from(runtime).join("bus")));
        }
    }
    found
}

/// A D-Bus address's value, its `%xx` escapes undone.
fn unescape(value: &str) -> Vec<u8> {
    let b = value.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("x"), 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

/// The connection's reading side, and what has arrived of a message not yet whole.
struct Conn {
    stream: UnixStream,
    inbox: Vec<u8>,
}

impl Conn {
    fn open() -> io::Result<Conn> {
        let address = std::env::var("DBUS_SESSION_BUS_ADDRESS").ok();
        let runtime = std::env::var("XDG_RUNTIME_DIR").ok();
        let mut last = io::Error::new(io::ErrorKind::NotFound, "no session bus address");
        for target in targets(address.as_deref(), runtime.as_deref()) {
            let stream = match &target {
                Target::Path(path) => UnixStream::connect(path),
                Target::Abstract(name) => connect_abstract(name),
            };
            match stream.and_then(authenticate) {
                Ok(stream) => return Ok(Conn { stream, inbox: Vec::new() }),
                Err(e) => last = e,
            }
        }
        Err(last)
    }

    /// The next whole message, or `None` when a second passed without one.
    fn next(&mut self) -> io::Result<Option<Msg>> {
        loop {
            if let Some(total) = frame_len(&self.inbox)? {
                if self.inbox.len() >= total {
                    let bytes: Vec<u8> = self.inbox.drain(..total).collect();
                    return parse(&bytes).map(Some).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e));
                }
            }
            let mut buffer = [0u8; 16384];
            match self.stream.read(&mut buffer) {
                Ok(0) => return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "the bus closed the connection")),
                Ok(n) => self.inbox.extend_from_slice(&buffer[..n]),
                Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => return Ok(None),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
    }
}

#[cfg(target_os = "linux")]
fn connect_abstract(name: &[u8]) -> io::Result<UnixStream> {
    use std::os::linux::net::SocketAddrExt;
    UnixStream::connect_addr(&std::os::unix::net::SocketAddr::from_abstract_name(name)?)
}

#[cfg(not(target_os = "linux"))]
fn connect_abstract(_name: &[u8]) -> io::Result<UnixStream> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "abstract sockets are Linux's"))
}

/// The `EXTERNAL` handshake: this process's uid, which the bus checks against the socket's own
/// word for who connected.
fn authenticate(mut stream: UnixStream) -> io::Result<UnixStream> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    // SAFETY: getuid cannot fail and touches no memory.
    let uid = unsafe { libc::getuid() }.to_string();
    let hex: String = uid.bytes().map(|b| format!("{b:02x}")).collect();
    stream.write_all(b"\0")?;
    stream.write_all(format!("AUTH EXTERNAL {hex}\r\n").as_bytes())?;
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    while !line.ends_with(b"\r\n") {
        if line.len() > 1024 || stream.read(&mut byte)? == 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "the bus said nothing sensible"));
        }
        line.push(byte[0]);
    }
    if !line.starts_with(b"OK ") {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("the bus refused us: {}", String::from_utf8_lossy(&line).trim()),
        ));
    }
    stream.write_all(b"BEGIN\r\n")?;
    Ok(stream)
}

/// Writing, little-endian, each value at its alignment from the start of the message (a body
/// starts on a multiple of 8, so from the body's start is the same).
#[derive(Default)]
struct W {
    b: Vec<u8>,
}

impl W {
    fn pad(&mut self, n: usize) {
        while self.b.len() % n != 0 {
            self.b.push(0);
        }
    }
    fn y(&mut self, v: u8) {
        self.b.push(v);
    }
    fn u(&mut self, v: u32) {
        self.pad(4);
        self.b.extend_from_slice(&v.to_le_bytes());
    }
    #[cfg(test)]
    fn i(&mut self, v: i32) {
        self.u(v as u32);
    }
    fn s(&mut self, v: &str) {
        self.u(v.len() as u32);
        self.b.extend_from_slice(v.as_bytes());
        self.b.push(0);
    }
    fn g(&mut self, v: &str) {
        self.b.push(v.len() as u8);
        self.b.extend_from_slice(v.as_bytes());
        self.b.push(0);
    }
    /// An array: its length in bytes, the padding to its first element (counted out of the
    /// length, and there even when it is empty), then what `elements` writes.
    fn array(&mut self, element_alignment: usize, elements: impl FnOnce(&mut W)) {
        self.pad(4);
        let at = self.b.len();
        self.b.extend_from_slice(&[0; 4]);
        self.pad(element_alignment);
        let start = self.b.len();
        elements(self);
        let length = (self.b.len() - start) as u32;
        self.b[at..at + 4].copy_from_slice(&length.to_le_bytes());
    }
}

/// Reading, in either byte order.
struct R<'a> {
    b: &'a [u8],
    p: usize,
    big: bool,
}

impl<'a> R<'a> {
    fn new(b: &'a [u8], big: bool) -> Self {
        R { b, p: 0, big }
    }
    fn align(&mut self, n: usize) -> Result<(), String> {
        let p = self.p.div_ceil(n) * n;
        if p > self.b.len() {
            return Err("the message ends early".into());
        }
        self.p = p;
        Ok(())
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.p.checked_add(n).filter(|e| *e <= self.b.len()).ok_or("the message ends early")?;
        let part = &self.b[self.p..end];
        self.p = end;
        Ok(part)
    }
    fn y(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> Result<u32, String> {
        self.align(4)?;
        let b: [u8; 4] = self.take(4)?.try_into().unwrap();
        Ok(if self.big { u32::from_be_bytes(b) } else { u32::from_le_bytes(b) })
    }
    fn i32(&mut self) -> Result<i32, String> {
        Ok(self.u32()? as i32)
    }
    fn s(&mut self) -> Result<String, String> {
        let n = self.u32()? as usize;
        let text = String::from_utf8_lossy(self.take(n)?).into_owned();
        self.take(1)?;
        Ok(text)
    }
    fn g(&mut self) -> Result<String, String> {
        let n = self.y()? as usize;
        let text = String::from_utf8_lossy(self.take(n)?).into_owned();
        self.take(1)?;
        Ok(text)
    }

    /// Step over values of these types, without understanding them.
    fn skip(&mut self, signature: &[u8]) -> Result<(), String> {
        let mut at = 0;
        while at < signature.len() {
            at = self.skip_one(signature, at, 0)?;
        }
        Ok(())
    }

    /// Step over one value of the complete type starting at `at`; where the next type starts.
    fn skip_one(&mut self, signature: &[u8], at: usize, depth: u32) -> Result<usize, String> {
        if depth > 64 {
            return Err("types nested too deep".into());
        }
        let c = *signature.get(at).ok_or("the signature ends early")?;
        match c {
            b'y' => {
                self.take(1)?;
            }
            b'n' | b'q' => {
                self.align(2)?;
                self.take(2)?;
            }
            b'b' | b'i' | b'u' | b'h' => {
                self.align(4)?;
                self.take(4)?;
            }
            b'x' | b't' | b'd' => {
                self.align(8)?;
                self.take(8)?;
            }
            b's' | b'o' => {
                self.s()?;
            }
            b'g' => {
                self.g()?;
            }
            b'v' => {
                let inner = self.g()?.into_bytes();
                if self.skip_one(&inner, 0, depth + 1)? != inner.len() {
                    return Err("a variant holds more than one value".into());
                }
            }
            b'a' => {
                // Its length says how far: the elements need not be looked at.
                let length = self.u32()? as usize;
                let element = *signature.get(at + 1).ok_or("an array of nothing")?;
                self.align(alignment(element))?;
                self.take(length)?;
                return end_of(signature, at + 1, depth + 1);
            }
            b'(' | b'{' => {
                let close = if c == b'(' { b')' } else { b'}' };
                self.align(8)?;
                let mut i = at + 1;
                while signature.get(i) != Some(&close) {
                    if i >= signature.len() {
                        return Err("an unclosed struct".into());
                    }
                    i = self.skip_one(signature, i, depth + 1)?;
                }
                return Ok(i + 1);
            }
            _ => return Err(format!("no such type {:?}", c as char)),
        }
        Ok(at + 1)
    }
}

/// Where the complete type starting at `at` ends.
fn end_of(signature: &[u8], at: usize, depth: u32) -> Result<usize, String> {
    if depth > 64 {
        return Err("types nested too deep".into());
    }
    match signature.get(at).ok_or("the signature ends early")? {
        b'a' => end_of(signature, at + 1, depth + 1),
        c @ (b'(' | b'{') => {
            let close = if *c == b'(' { b')' } else { b'}' };
            let mut i = at + 1;
            while signature.get(i) != Some(&close) {
                if i >= signature.len() {
                    return Err("an unclosed struct".into());
                }
                i = end_of(signature, i, depth + 1)?;
            }
            Ok(i + 1)
        }
        _ => Ok(at + 1),
    }
}

fn alignment(c: u8) -> usize {
    match c {
        b'y' | b'g' | b'v' => 1,
        b'n' | b'q' => 2,
        b'x' | b't' | b'd' | b'(' | b'{' => 8,
        _ => 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Notify`'s body as an application writes it, with a picture among the hints.
    fn notify_body(actions: &[&str], timeout: i32) -> Vec<u8> {
        let mut w = W::default();
        w.s("Firefox");
        w.u(0);
        w.s("firefox");
        w.s("Download finished");
        w.s("film.mkv <b>is</b> ready &amp; waiting");
        w.array(4, |w| {
            for a in actions {
                w.s(a);
            }
        });
        w.array(8, |w| {
            w.pad(8);
            w.s("urgency");
            w.g("y");
            w.y(1);
            w.pad(8);
            w.s("image-data");
            w.g("(iiibiiay)");
            w.pad(8);
            for v in [2, 1, 8] {
                w.i(v);
            }
            w.u(1);
            w.i(8);
            w.i(4);
            w.array(1, |w| w.b.extend_from_slice(&[9; 8]));
            w.pad(8);
            w.s("desktop-entry");
            w.g("s");
            w.s("firefox");
            w.pad(8);
            w.s("x-nested");
            w.g("v");
            w.g("as");
            w.array(4, |w| w.s("deep"));
        });
        w.i(timeout);
        w.b
    }

    #[test]
    fn notify_is_read_past_any_hints() {
        let n = read_notify(&notify_body(&["default", "Open", "later", "Later"], 5000), false).unwrap();
        assert_eq!(
            n,
            Notify {
                app: "Firefox".into(),
                replaces: 0,
                summary: "Download finished".into(),
                body: "film.mkv <b>is</b> ready &amp; waiting".into(),
                actions: vec!["default".into(), "Open".into(), "later".into(), "Later".into()],
                timeout: 5000,
            }
        );
        let n = read_notify(&notify_body(&[], -1), false).unwrap();
        assert!(n.actions.is_empty());
        assert_eq!(n.timeout, -1);
    }

    #[test]
    fn a_short_or_wrong_body_is_refused_not_panicked_on() {
        let body = notify_body(&[], 0);
        for cut in [0, 3, 20, body.len() - 1] {
            assert!(read_notify(&body[..cut], false).is_err(), "cut at {cut}");
        }
        let mut r = R::new(&[1, 0, 0, 0, 0], false);
        assert!(r.skip(b"(iz)").is_err());
        let mut r = R::new(&[], false);
        assert!(r.skip(b"a{sv").is_err());
    }

    #[test]
    fn a_message_round_trips_through_the_wire() {
        let mut w = W::default();
        w.u(7);
        w.s("default");
        let header = Header {
            path: Some(PATH),
            interface: Some(NAME),
            member: Some("ActionInvoked"),
            reply_serial: Some(41),
            destination: Some(":1.9"),
            ..Header::default()
        };
        let bytes = encode(SIGNAL, 0, 42, &header, "us", &w.b);
        assert_eq!(frame_len(&bytes[..15]).unwrap(), None);
        assert_eq!(frame_len(&bytes).unwrap(), Some(bytes.len()));
        let m = parse(&bytes).unwrap();
        assert_eq!((m.kind, m.serial, m.reply_serial), (SIGNAL, 42, Some(41)));
        assert_eq!(m.path.as_deref(), Some(PATH));
        assert_eq!(m.interface.as_deref(), Some(NAME));
        assert_eq!(m.member.as_deref(), Some("ActionInvoked"));
        assert_eq!(m.signature, "us");
        let mut r = R::new(&m.body, m.big);
        assert_eq!((r.u32().unwrap(), r.s().unwrap()), (7, "default".to_string()));
        // An empty body has no signature field and nothing after the header.
        let bytes = encode(RETURN, 1, 5, &Header { reply_serial: Some(4), ..Header::default() }, "", &[]);
        assert_eq!(bytes.len() % 8, 0);
        let m = parse(&bytes).unwrap();
        assert!(m.body.is_empty() && m.signature.is_empty());
    }

    #[test]
    fn a_big_endian_message_is_read() {
        // A method call, serial 3, member "Ping", no body, written big-endian by hand.
        let mut b = vec![b'B', CALL, 0, 1, 0, 0, 0, 0, 0, 0, 0, 3];
        let mut fields = vec![3, 1, b's', 0, 0, 0, 0, 4];
        fields.extend_from_slice(b"Ping\0");
        b.extend_from_slice(&(fields.len() as u32).to_be_bytes());
        b.extend_from_slice(&fields);
        while b.len() % 8 != 0 {
            b.push(0);
        }
        assert_eq!(frame_len(&b).unwrap(), Some(b.len()));
        let m = parse(&b).unwrap();
        assert!(m.big);
        assert_eq!((m.kind, m.serial, m.member.as_deref()), (CALL, 3, Some("Ping")));
        let mut r = R::new(&[0, 0, 1, 2, 0, 0, 0, 2, b'h', b'i', 0], true);
        assert_eq!(r.u32().unwrap(), 258);
        assert_eq!(r.s().unwrap(), "hi");
    }

    #[test]
    fn how_long_a_card_stays() {
        assert_eq!(lasting(-1), DEFAULT_FOR);
        assert_eq!(lasting(-5), DEFAULT_FOR);
        assert_eq!(lasting(0), NEVER_FOR);
        assert_eq!(lasting(100), SHORTEST);
        assert_eq!(lasting(5000), Duration::from_secs(5));
        assert_eq!(lasting(i32::MAX), LONGEST);
    }

    #[test]
    fn the_bus_is_found_where_its_address_says() {
        let found = targets(Some("unix:abstract=/tmp/dbus-x,guid=1234;unix:path=/run/user/1000/my%20bus;tcp:host=x"), Some("/run/user/1000"));
        assert!(matches!(&found[0], Target::Abstract(n) if n == b"/tmp/dbus-x"));
        assert!(matches!(&found[1], Target::Path(p) if p == std::path::Path::new("/run/user/1000/my bus")));
        assert_eq!(found.len(), 2);
        let found = targets(None, Some("/run/user/1000"));
        assert!(matches!(&found[0], Target::Path(p) if p == std::path::Path::new("/run/user/1000/bus")));
        assert!(targets(Some("tcp:host=x"), None).is_empty());
        assert_eq!(unescape("a%2fb%zz%4"), b"a/b%zz%4");
    }

    /// Against a real bus, with nothing but this module: run with a private bus's address in
    /// `DBUS_SESSION_BUS_ADDRESS` and `--ignored --nocapture`, and send it notifications. It
    /// prints each card as it changes and presses any card that says "press me".
    #[test]
    #[ignore]
    fn serves_a_real_bus() {
        let _ = env_logger::builder().is_test(true).filter_level(log::LevelFilter::Info).try_init();
        start();
        let secs: u64 = std::env::var("SPATIAND_NOTIFY_TEST_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(30);
        let until = Instant::now() + Duration::from_secs(secs);
        let mut was = String::new();
        while Instant::now() < until {
            let now = notice();
            if now != was {
                eprintln!("card: {now:?} (showing {})", showing());
                if now.contains("press me") {
                    pressed();
                }
                was = now;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}
