//! A pairing in progress, on its own thread.
//!
//! The protocol is in `spatiand_stream::link::pair` and `spatiand_stream::pairing`; this only
//! runs it off the compositor thread and keeps what it learns where the compositor can look.
//! The compositor polls -- a pairing takes a round trip or two once the code is typed, and a
//! poll per frame is nothing next to that.

use std::sync::{Arc, Mutex};

use spatiand_stream::link::{self, Paired, Pairing as Progress};
use spatiand_stream::{Fingerprint, Identity};

/// What the compositor can see of a pairing.
#[derive(Debug, Clone, Default)]
pub struct State {
    /// The certificate the host presented, once connected: the code is being proved against it.
    pub fingerprint: Option<Fingerprint>,
    /// Set once, when it is over.
    pub outcome: Option<Result<Paired, String>>,
}

pub struct Pairing {
    pub address: String,
    state: Arc<Mutex<State>>,
    /// Dropping this ends the attempt, and the connection with it.
    _cancel: tokio::sync::oneshot::Sender<()>,
}

impl Pairing {
    /// Pair with `address`, proving `code`, the digits the wearer typed from the host's screen.
    pub fn start(address: String, code: String, identity_dir: std::path::PathBuf) -> Pairing {
        let state: Arc<Mutex<State>> = Arc::default();
        let (cancel, cancelled) = tokio::sync::oneshot::channel::<()>();
        {
            let state = state.clone();
            let address = address.clone();
            let _ = std::thread::Builder::new()
                .name(format!("pair-{address}"))
                .spawn(move || {
                    let finish = |outcome: Result<Paired, String>| {
                        if let Ok(mut s) = state.lock() {
                            s.outcome = Some(outcome);
                        }
                    };
                    let identity = match Identity::load_or_create(&identity_dir) {
                        Ok(identity) => identity,
                        Err(e) => return finish(Err(e)),
                    };
                    let runtime = match tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                    {
                        Ok(runtime) => runtime,
                        Err(e) => return finish(Err(format!("no network runtime: {e}"))),
                    };
                    let outcome = runtime.block_on(async {
                        let progress = |p: Progress| {
                            let Progress::Proving { fingerprint } = p;
                            log::info!("pairing with {address}: it is {}; proving the code", fingerprint.short());
                            if let Ok(mut s) = state.lock() {
                                s.fingerprint = Some(fingerprint);
                            }
                        };
                        tokio::select! {
                            outcome = link::pair(&identity, &address, &code, progress) => outcome,
                            _ = cancelled => Err("cancelled".into()),
                        }
                    });
                    match &outcome {
                        Ok(paired) => log::info!("{address} and this device proved the code; it is {}", paired.name),
                        Err(e) => log::info!("pairing with {address} ended: {e}"),
                    }
                    finish(outcome);
                });
        }
        Pairing {
            address,
            state,
            _cancel: cancel,
        }
    }

    pub fn state(&self) -> State {
        self.state.lock().map(|s| s.clone()).unwrap_or_default()
    }
}
