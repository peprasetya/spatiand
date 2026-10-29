//! The service's local socket, and the client every caller uses.
//!
//! STUB: the signatures are the contract; the bodies are to be written.

use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::service::Service;
use crate::wire::{Refusal, Request};

/// Serve `service` on the socket at `path` (an old socket file there is replaced), one thread per
/// connection. Only this user's processes are answered (the peer's uid is checked); the asking
/// program's name is read from the peer process and put in `who.app`, with what the caller said
/// of itself after it ("spatikey-host (Chrome)").
pub fn serve(path: &Path, service: Arc<Service>) -> std::io::Result<std::thread::JoinHandle<()>> {
    let _ = (path, service);
    todo!()
}

pub struct Client {
    #[allow(dead_code)]
    stream: UnixStream,
    #[allow(dead_code)]
    next: u64,
    #[allow(dead_code)]
    app: String,
}

impl Client {
    /// Connect to the service at [`crate::paths::socket`], naming the caller `app`.
    pub fn connect(app: &str) -> std::io::Result<Client> {
        Client::connect_to(&crate::paths::socket(), app)
    }

    pub fn connect_to(path: &Path, app: &str) -> std::io::Result<Client> {
        let _ = (path, app);
        todo!()
    }

    /// Ask, and wait for the answer (a person may be asked first: no time limit here).
    pub fn ask(&mut self, request: Request) -> Result<Value, Refusal> {
        let _ = request;
        todo!()
    }

    pub fn ask_as<T: DeserializeOwned>(&mut self, request: Request) -> Result<T, Refusal> {
        let value = self.ask(request)?;
        serde_json::from_value(value)
            .map_err(|e| Refusal::new(crate::wire::Code::Failed, format!("an answer this program cannot read: {e}")))
    }

    /// Send a raw JSON request as the browsers' host relays it, and return the raw JSON answer.
    pub fn relay(&mut self, json: &[u8]) -> std::io::Result<Vec<u8>> {
        let _ = json;
        todo!()
    }
}
