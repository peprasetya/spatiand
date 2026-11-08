//! The service's local socket, and the client every caller uses.
//!
//! The socket is a file only this user can reach (0600, in a folder made 0700), and each
//! connection is checked again by the peer's user id, which the kernel gives and the caller
//! cannot choose. The asking program is named from the peer's process, not from what it says of
//! itself -- except for the browsers' host, which speaks for a browser the kernel cannot see.

#[cfg(feature = "service")]
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, PermissionsExt};
#[cfg(feature = "service")]
use std::os::unix::io::AsRawFd;
#[cfg(feature = "service")]
use std::os::unix::net::UnixListener;
use std::os::unix::net::UnixStream;
use std::path::Path;
#[cfg(feature = "service")]
use std::sync::Arc;
#[cfg(feature = "service")]
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde_json::Value;
use zeroize::Zeroizing;

#[cfg(feature = "service")]
use crate::service::{self, Service};
use crate::wire::{self, Code, Envelope, Refusal, Reply, Request, Who};

#[cfg(feature = "service")]
/// Spatiand's own programs, by the name they run as, and as a prompt names them.
const OWN: &[(&str, &str)] = &[
    ("spatikey", "SpatiKey"),
    ("spatimesh", "SpatiMesh"),
    // SpatiMesh's Chromium as the Beam Pro's APK carries it (crates/spatiand/src/own.rs).
    ("spatimesh_chromium", "SpatiMesh"),
    ("spatipal", "SpatiPal"),
    ("spatiquill", "SpatiQuill"),
    ("spatishelf", "SpatiShelf"),
    ("spatireel", "SpatiReel"),
    ("spatiloupe", "SpatiLoupe"),
    ("spatiand", "Spatiand"),
];

#[cfg(feature = "service")]
/// The browsers' native-messaging host: it relays for an extension, and says which browser.
const HOST: &str = "spatikey-host";

#[cfg(feature = "service")]
/// Serve `service` on the socket at `path` (an old socket file there is replaced), one thread per
/// connection. Only this user's processes are answered (the peer's uid is checked). The asking
/// program's name is read from the peer process and put in `who.app` ("SpatiMesh"); for the
/// browsers' host it is the browser the host names ("Chrome"), as nothing else knows it.
/// `who.device` is always emptied: whoever is on this socket is on this machine.
pub fn serve(path: &Path, service: Arc<Service>) -> std::io::Result<std::thread::JoinHandle<()>> {
    if let Some(dir) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
        if !dir.is_dir() {
            std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
        }
    }
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        if !meta.file_type().is_socket() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!("{} is there and is not a socket", path.display()),
            ));
        }
        // An old socket answers nobody; a live one is another service's, and is left to it.
        if UnixStream::connect(path).is_ok() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AddrInUse,
                format!("another SpatiKey service answers at {}", path.display()),
            ));
        }
        std::fs::remove_file(path)?;
    }
    let listener = UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    log::info!("spatikey: answering on {}", path.display());
    std::thread::Builder::new().name("spatikey-socket".into()).spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => {
                    let service = service.clone();
                    let spawned =
                        std::thread::Builder::new().name("spatikey-caller".into()).spawn(move || serve_one(stream, service));
                    if let Err(e) = spawned {
                        log::warn!("spatikey: no thread for a caller: {e}");
                    }
                }
                Err(e) => {
                    // Out of files, most likely: wait for some to close rather than spin.
                    log::warn!("spatikey: could not take a caller: {e}");
                    std::thread::sleep(Duration::from_millis(200));
                }
            }
        }
    })
}

#[cfg(feature = "service")]
/// Answer one connection until it closes.
fn serve_one(mut stream: UnixStream, service: Arc<Service>) {
    let Some(peer) = peer(&stream) else {
        log::warn!("spatikey: a caller whose user cannot be told; not answered");
        return;
    };
    // SAFETY: geteuid cannot fail.
    let me = unsafe { libc::geteuid() };
    if peer.uid != me {
        log::warn!("spatikey: refused a caller of user {}", peer.uid);
        return;
    }
    let program = peer.pid.and_then(program_of);
    let conn = service::next_conn();
    loop {
        // Too large, broken off or closed: the connection ends.
        let Ok(frame) = wire::read_frame(&mut stream).map(Zeroizing::new) else { break };
        let reply = match parse(&frame) {
            Ok(mut env) => {
                env.who.app = caller(program.as_deref(), &env.who.app);
                env.who.device.clear();
                service.handle(conn, env)
            }
            Err(refusal) => refusal,
        };
        let json = match serde_json::to_vec(&reply) {
            Ok(json) => Zeroizing::new(json),
            Err(e) => {
                let failed = Reply::err(reply.id, Refusal::new(Code::Failed, format!("could not write the answer: {e}")));
                Zeroizing::new(serde_json::to_vec(&failed).unwrap_or_default())
            }
        };
        if wire::write_frame(&mut stream, &json).is_err() {
            break;
        }
    }
    service.closed(conn);
}

#[cfg(feature = "service")]
/// A request, or the refusal to send back: with id 0 when the message is not JSON at all, else
/// with its own id, so that the caller knows which question was not understood.
fn parse(frame: &[u8]) -> Result<Envelope, Reply> {
    let value: Value =
        serde_json::from_slice(frame).map_err(|_| Reply::err(0, Refusal::new(Code::BadRequest, "that is not JSON")))?;
    let id = value.get("id").and_then(Value::as_u64).unwrap_or(0);
    serde_json::from_value(value)
        .map_err(|e| Reply::err(id, Refusal::new(Code::BadRequest, format!("not a request SpatiKey knows: {e}"))))
}

#[cfg(feature = "service")]
/// Who is asking, as the prompt names them: one of Spatiand's own programs by its name, the
/// browser the host speaks for, or any other program as it is called.
fn caller(program: Option<&str>, claimed: &str) -> String {
    match program {
        Some(program) if program == HOST => {
            let claimed: String = claimed.chars().filter(|c| !c.is_control()).collect();
            match claimed.trim() {
                "" => "a browser".to_string(),
                browser => browser.to_string(),
            }
        }
        Some(program) => OWN
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(program))
            .map(|(_, shown)| shown.to_string())
            .unwrap_or_else(|| program.to_string()),
        None => "an unknown program".to_string(),
    }
}

#[cfg(feature = "service")]
/// The peer of a connection, as the kernel tells it.
struct Peer {
    uid: u32,
    pid: Option<i32>,
}

#[cfg(feature = "service")]
#[cfg(any(target_os = "macos", target_os = "ios"))]
fn peer(stream: &UnixStream) -> Option<Peer> {
    let fd = stream.as_raw_fd();
    let (mut uid, mut gid): (libc::uid_t, libc::gid_t) = (0, 0);
    // SAFETY: fd is the open socket, and both pointers are to locals of the right types.
    if unsafe { libc::getpeereid(fd, &mut uid, &mut gid) } != 0 {
        return None;
    }
    let mut pid: libc::pid_t = 0;
    let mut length = std::mem::size_of::<libc::pid_t>() as libc::socklen_t;
    // SAFETY: LOCAL_PEERPID writes one pid_t, the size given.
    let found = unsafe {
        libc::getsockopt(fd, libc::SOL_LOCAL, libc::LOCAL_PEERPID, (&mut pid as *mut libc::pid_t).cast(), &mut length)
    };
    Some(Peer { uid, pid: (found == 0 && pid > 0).then_some(pid) })
}

#[cfg(feature = "service")]
#[cfg(any(target_os = "linux", target_os = "android"))]
fn peer(stream: &UnixStream) -> Option<Peer> {
    /// `struct ucred`, the same on Linux and Android (libc keeps Android's elsewhere).
    #[repr(C)]
    struct Ucred {
        pid: libc::pid_t,
        uid: libc::uid_t,
        gid: libc::gid_t,
    }
    let mut credentials = Ucred { pid: 0, uid: 0, gid: 0 };
    let mut length = std::mem::size_of::<Ucred>() as libc::socklen_t;
    // SAFETY: SO_PEERCRED writes one struct ucred, the size given.
    let found = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut credentials as *mut Ucred).cast(),
            &mut length,
        )
    };
    if found != 0 || length as usize != std::mem::size_of::<Ucred>() {
        return None;
    }
    Some(Peer { uid: credentials.uid, pid: (credentials.pid > 0).then_some(credentials.pid) })
}

#[cfg(feature = "service")]
#[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "linux", target_os = "android")))]
fn peer(_stream: &UnixStream) -> Option<Peer> {
    // No way to tell who is calling here, so nobody is answered.
    None
}

#[cfg(feature = "service")]
#[cfg(target_os = "macos")]
fn program_of(pid: i32) -> Option<String> {
    // PROC_PIDPATHINFO_MAXSIZE.
    let mut buffer = [0u8; 4096];
    // SAFETY: the buffer is as long as said.
    let n = unsafe { libc::proc_pidpath(pid, buffer.as_mut_ptr().cast(), buffer.len() as u32) };
    if n <= 0 {
        return None;
    }
    program_name(&String::from_utf8_lossy(&buffer[..n as usize]))
}

#[cfg(feature = "service")]
#[cfg(not(target_os = "macos"))]
fn program_of(pid: i32) -> Option<String> {
    std::fs::read_link(format!("/proc/{pid}/exe"))
        .ok()
        .and_then(|path| program_name(&path.to_string_lossy()))
        .or_else(|| std::fs::read_to_string(format!("/proc/{pid}/comm")).ok().and_then(|comm| program_name(comm.trim())))
}

#[cfg(feature = "service")]
/// A program's name from its path: the file's name, without what Linux adds to a file replaced
/// since, and on Android without the `lib….so` that Spatiand's programs are carried as there.
fn program_name(path: &str) -> Option<String> {
    let name = path.rsplit('/').next()?.trim_end_matches(" (deleted)");
    let name = name.strip_prefix("lib").and_then(|n| n.strip_suffix(".so")).unwrap_or(name);
    (!name.is_empty()).then(|| name.to_string())
}

pub struct Client {
    stream: UnixStream,
    next: u64,
    app: String,
}

impl Client {
    /// Connect to the service at [`crate::paths::socket`], naming the caller `app`.
    pub fn connect(app: &str) -> std::io::Result<Client> {
        Client::connect_to(&crate::paths::socket(), app)
    }

    pub fn connect_to(path: &Path, app: &str) -> std::io::Result<Client> {
        let stream = UnixStream::connect(path)?;
        Ok(Client { stream, next: 1, app: app.to_string() })
    }

    /// Ask, and wait for the answer (a person may be asked first: no time limit here).
    pub fn ask(&mut self, request: Request) -> Result<Value, Refusal> {
        let id = self.next;
        self.next += 1;
        let env = Envelope { id, who: Who { app: self.app.clone(), device: String::new() }, request };
        let json = Zeroizing::new(
            serde_json::to_vec(&env).map_err(|e| Refusal::new(Code::BadRequest, format!("could not write the request: {e}")))?,
        );
        drop(env);
        let answer = self
            .relay(&json)
            .map(Zeroizing::new)
            .map_err(|e| Refusal::new(Code::Failed, format!("the SpatiKey service did not answer: {e}")))?;
        let reply: Reply = serde_json::from_slice(&answer)
            .map_err(|_| Refusal::new(Code::Failed, "the SpatiKey service answered something this program cannot read"))?;
        // Id 0 answers what the service could not read as a request at all.
        if reply.id != id && reply.id != 0 {
            return Err(Refusal::new(Code::Failed, "the SpatiKey service answered another question"));
        }
        reply.into_result()
    }

    pub fn ask_as<T: DeserializeOwned>(&mut self, request: Request) -> Result<T, Refusal> {
        let value = self.ask(request)?;
        serde_json::from_value(value)
            .map_err(|e| Refusal::new(crate::wire::Code::Failed, format!("an answer this program cannot read: {e}")))
    }

    /// Send a raw JSON request as the browsers' host relays it, and return the raw JSON answer.
    pub fn relay(&mut self, json: &[u8]) -> std::io::Result<Vec<u8>> {
        wire::write_frame(&mut self.stream, json)?;
        wire::read_frame(&mut self.stream)
    }
}

#[cfg(feature = "service")]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callers_are_named_by_their_program() {
        assert_eq!(caller(Some("spatimesh"), "Chrome"), "SpatiMesh", "what a program says of itself is not taken");
        assert_eq!(caller(Some("spatimesh_chromium"), "Chrome"), "SpatiMesh", "the Beam Pro's");
        assert_eq!(caller(Some("spatikey"), ""), "SpatiKey");
        assert_eq!(caller(Some(HOST), "Chrome"), "Chrome");
        assert_eq!(caller(Some(HOST), " \n"), "a browser");
        assert_eq!(caller(Some("python3"), "SpatiKey"), "python3");
        assert_eq!(caller(None, "SpatiKey"), "an unknown program");
    }

    #[test]
    fn program_names_come_from_paths() {
        assert_eq!(program_name("/Applications/Spatiand.app/Contents/MacOS/spatikey-host").as_deref(), Some(HOST));
        assert_eq!(program_name("/usr/lib/spatiand/spatimesh (deleted)").as_deref(), Some("spatimesh"));
        assert_eq!(program_name("/data/app/lib/arm64/libspatimesh.so").as_deref(), Some("spatimesh"));
        assert_eq!(program_name("/usr/lib/libc.so.6").as_deref(), Some("libc.so.6"));
        assert_eq!(program_name("/"), None);
    }

    #[test]
    fn what_is_not_a_request_is_answered_with_its_id() {
        let not_json = parse(b"{oops").unwrap_err();
        assert_eq!((not_json.id, not_json.err.unwrap().code), (0, Code::BadRequest));
        let unknown = parse(br#"{"id":9,"op":"open_sesame"}"#).unwrap_err();
        assert_eq!((unknown.id, unknown.err.unwrap().code), (9, Code::BadRequest));
        assert!(matches!(parse(br#"{"id":3,"op":"status"}"#).unwrap().request, Request::Status));
    }
}
