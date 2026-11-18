//! Asking the service, on a thread of its own.
//!
//! A request may wait on a finger for as long as the wearer takes -- or on a Mac across the room
//! -- and meanwhile the window has to go on being drawn and answering the room. So requests go to
//! one thread that holds the connection, and their answers come back on a channel the window's
//! loop looks at, with a nudge down a pipe it watches so that it looks at once.
//!
//! One connection, one request at a time, in the order asked: a session belongs to the
//! connection that opened it, and the service answers a connection's requests in turn anyway.

use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::time::{Duration, Instant};

use serde_json::Value;
use spatikey_core::socket::Client;
use spatikey_core::wire::{Code, Found, Refusal, Request};

/// What a request was for: how its answer is read, and whether it is still wanted when it comes.
#[derive(Debug, Clone, PartialEq)]
pub enum Asked {
    /// `route`: the screen follows from the answer (after opening, setting up, pairing). Without
    /// it, only what is known is brought up to date -- whether the Mac is near.
    Status { route: bool },
    Setup,
    Open,
    Close,
    List,
    /// One entry, by its id.
    Get(String),
    Put,
    Delete,
    /// An entry's current code, by its id.
    Totp(String),
    /// Typing, into the window of this title.
    Type(String),
    Target,
    RecoveryNew,
    /// The recovery code shown was said to be saved.
    RecoverySaved,
    Recover,
    Devices,
    Forget,
    Settings,
    Find,
    PairWith(Found),
    PairConfirm(bool),
    /// This device asking to be added as a holder.
    HolderRequest,
    /// Adding a device that asked.
    Approve,
    /// Turning its request down.
    TurnDown,
}

/// Why there is no answer.
#[derive(Debug, Clone)]
pub enum Trouble {
    /// The service said no, and why.
    Refused(Refusal),
    /// There is no service to ask: it is not running on this computer.
    Away(String),
}

/// What answers: SpatiKey's service, or `--demo`'s stand-in for it.
pub trait Service: Send {
    fn ask(&mut self, request: Request) -> Result<Value, Trouble>;
}

/// The service, over its socket. Connected when first asked, and again after the connection
/// fails (the service restarted with Spatiand) -- not after the service's own refusals, which
/// leave it as it was. A session goes with its connection: a request made in one when the
/// connection fails is answered `bad_session` here, and the window goes back to its locked screen.
pub struct Socket {
    client: Option<Client>,
    path: std::path::PathBuf,
}

impl Socket {
    pub fn new() -> Socket {
        Socket { client: None, path: spatikey_core::paths::socket() }
    }
}

impl Service for Socket {
    fn ask(&mut self, request: Request) -> Result<Value, Trouble> {
        let client = match self.client.as_mut() {
            Some(client) => client,
            None => match Client::connect_to(&self.path, "SpatiKey") {
                Ok(client) => self.client.insert(client),
                Err(e) => {
                    log::info!("SpatiKey's service is not there: {e}");
                    return Err(Trouble::Away(e.to_string()));
                }
            },
        };
        let in_session = carries_session(&request);
        match client.try_ask(request) {
            Ok(answer) => answer.map_err(Trouble::Refused),
            // The connection failed: asked again on a new one. A session went with it -- the
            // service started again -- so what was asked in one is told so at once, and the
            // window locks, rather than every later request failing.
            Err(lost) => {
                self.client = None;
                log::info!("SpatiKey's service went: {}", lost.message);
                if in_session {
                    Err(Trouble::Refused(Refusal::new(Code::BadSession, "the SpatiKey service started again")))
                } else {
                    Err(Trouble::Refused(lost))
                }
            }
        }
    }
}

/// Whether a request is made in a session (and so goes with the connection it was opened on).
fn carries_session(request: &Request) -> bool {
    match request {
        Request::Close { .. }
        | Request::List { .. }
        | Request::Get { .. }
        | Request::Put { .. }
        | Request::Delete { .. }
        | Request::RecoveryNew { .. }
        | Request::Devices { .. }
        | Request::ForgetDevice { .. }
        | Request::HolderApprove { .. }
        | Request::HolderRefuse { .. }
        | Request::Settings { .. } => true,
        Request::Totp { session, .. } | Request::Reveal { session, .. } | Request::Type { session, .. } => session.is_some(),
        _ => false,
    }
}

/// A request's name, for the log: never the request itself, whose fields may be a password or
/// the recovery code.
pub fn op(request: &Request) -> &'static str {
    match request {
        Request::Status => "status",
        Request::Setup { .. } => "setup",
        Request::Open => "open",
        Request::Close { .. } => "close",
        Request::List { .. } => "list",
        Request::Get { .. } => "get",
        Request::Put { .. } => "put",
        Request::Delete { .. } => "delete",
        Request::Totp { .. } => "totp",
        Request::Reveal { .. } => "reveal",
        Request::Fill { .. } => "fill",
        Request::Save { .. } => "save",
        Request::ProfileUnlock { .. } => "profile_unlock",
        Request::PasskeyCreate(_) => "passkey_create",
        Request::PasskeyGet(_) => "passkey_get",
        Request::Type { .. } => "type",
        Request::Target => "target",
        Request::RecoveryNew { .. } => "recovery_new",
        Request::Recover { .. } => "recover",
        Request::RecoverySaved { .. } => "recovery_saved",
        Request::Devices { .. } => "devices",
        Request::ForgetDevice { .. } => "forget_device",
        Request::Settings { .. } => "settings",
        Request::Pair { .. } => "pair",
        Request::Find => "find",
        Request::PairWith { .. } => "pair_with",
        Request::PairConfirm { .. } => "pair_confirm",
        Request::HolderRequest { .. } => "holder_request",
        Request::HolderApprove { .. } => "holder_approve",
        Request::HolderRefuse { .. } => "holder_refuse",
    }
}

pub struct Answer {
    pub asked: Asked,
    pub result: Result<Value, Trouble>,
}

/// The thread that asks, as the window holds it.
pub struct Asker {
    jobs: Sender<(Asked, Request)>,
    answers: Receiver<Answer>,
    /// How many requests are asked and not yet answered.
    pub out: usize,
}

impl Asker {
    /// Start the thread. `wake` is called after each answer, from that thread.
    pub fn start(mut service: Box<dyn Service>, wake: impl Fn() + Send + 'static) -> Asker {
        let (jobs, queue) = channel::<(Asked, Request)>();
        let (done, answers) = channel();
        let _ = std::thread::Builder::new().name("asking".into()).spawn(move || {
            for (asked, request) in queue {
                let name = op(&request);
                let result = service.ask(request);
                match &result {
                    Ok(_) => log::debug!("{name}: answered"),
                    // The code only: a refusal's words are the service's, and the log is no
                    // place to wonder whether they quote anything.
                    Err(Trouble::Refused(refusal)) => log::info!("{name}: refused ({:?})", refusal.code),
                    Err(Trouble::Away(_)) => {}
                }
                if done.send(Answer { asked, result }).is_err() {
                    return;
                }
                wake();
            }
        });
        Asker { jobs, answers, out: 0 }
    }

    pub fn ask(&mut self, asked: Asked, request: Request) {
        log::debug!("asking: {}", op(&request));
        if self.jobs.send((asked, request)).is_ok() {
            self.out += 1;
        }
    }

    /// The answers come so far; None once the thread is gone, which nothing but a fault in it
    /// ends (and then nothing is answered again).
    pub fn answers(&mut self) -> Option<Vec<Answer>> {
        let mut got = Vec::new();
        loop {
            match self.answers.try_recv() {
                Ok(answer) => {
                    self.out = self.out.saturating_sub(1);
                    got.push(answer);
                }
                Err(TryRecvError::Empty) => return Some(got),
                Err(TryRecvError::Disconnected) => return if got.is_empty() { None } else { Some(got) },
            }
        }
    }

    /// Ask, and wait up to `wait` for that answer alone, dropping any others: what the window
    /// does last, closing its session as it goes.
    pub fn finish(&mut self, asked: Asked, request: Request, wait: Duration) {
        self.ask(asked.clone(), request);
        let until = Instant::now() + wait;
        loop {
            match self.answers.recv_timeout(until.saturating_duration_since(Instant::now())) {
                Ok(answer) if answer.asked == asked => return,
                Ok(_) => {}
                Err(RecvTimeoutError::Timeout) | Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spatikey_core::wire::{read_frame, write_frame};
    use std::os::unix::net::UnixListener;

    /// A service that refuses the first request on a connection as `failed` and goes with the
    /// second: the service's own refusal keeps the connection, its going drops it, and a session's
    /// request is then told the session is gone.
    #[test]
    fn a_lost_connection_is_told_from_a_refusal() {
        let dir = std::env::temp_dir().join(format!("spatikey-ask-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("socket");
        let listener = UnixListener::bind(&path).unwrap();
        let server = std::thread::spawn(move || {
            let mut accepted = 0;
            for stream in listener.incoming().take(2) {
                let mut stream = stream.unwrap();
                accepted += 1;
                let asked: Value = serde_json::from_slice(&read_frame(&mut stream).unwrap()).unwrap();
                let id = asked["id"].as_u64().unwrap();
                let refused = format!(r#"{{"id":{id},"err":{{"code":"failed","message":"no"}}}}"#);
                write_frame(&mut stream, refused.as_bytes()).unwrap();
                // The second request on it: the service goes.
                let _ = read_frame(&mut stream);
            }
            accepted
        });
        let mut socket = Socket { client: None, path };
        let refused = socket.ask(Request::Status);
        assert!(matches!(refused, Err(Trouble::Refused(ref r)) if r.code == Code::Failed && r.message == "no"), "{refused:?}");
        assert!(socket.client.is_some(), "a refusal leaves the connection");
        let lost = socket.ask(Request::List { session: "s".into() });
        assert!(matches!(lost, Err(Trouble::Refused(ref r)) if r.code == Code::BadSession), "{lost:?}");
        assert!(socket.client.is_none());
        // Asked again on a new connection.
        let again = socket.ask(Request::Status);
        assert!(matches!(again, Err(Trouble::Refused(ref r)) if r.code == Code::Failed), "{again:?}");
        drop(socket);
        assert_eq!(server.join().unwrap(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
