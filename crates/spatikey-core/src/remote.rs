//! The Deck and the Beam Pro asking the Mac. docs/spatikey.md, "The Deck and the Beam Pro ask
//! the Mac".
//!
//! STUB: the signatures are the contract; the bodies are to be written.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use spatiand_stream::Identity;

use crate::service::Service;
use crate::wire::{Approver, Device, Envelope, Found, PairAnswer, Refusal, Reply};

/// The Mac's side, running: answering broadcasts on [`crate::lan::DISCOVERY_PORT`] and requests on
/// [`crate::lan::ASK_PORT`]. Dropping it stops both.
pub struct Listening {
    #[allow(dead_code)]
    stop: Option<std::sync::mpsc::Sender<()>>,
}

/// Start the Mac's side for `service` with this Mac's link identity. Requests are answered only
/// for devices in `service.allowed()`, only from a LAN address heard broadcasting within two
/// minutes, each checked again per request; a device that broadcast `PAIR` may connect once,
/// for a minute, to ask `pair`.
pub fn serve(service: Arc<Service>, identity: Identity) -> Result<Listening, String> {
    let _ = (service, identity);
    todo!()
}

/// A device's side: finding its Macs and asking them.
pub struct Asker {
    #[allow(dead_code)]
    identity: Identity,
    #[allow(dead_code)]
    local: PathBuf,
    #[allow(dead_code)]
    name: String,
}

impl Asker {
    /// `local` is `local/spatikey`, where `approvers.json` is kept; `name` is this device's.
    pub fn new(identity: Identity, local: PathBuf, name: String) -> Asker {
        Asker { identity, local, name }
    }

    /// The Macs this device has paired with.
    pub fn approvers(&self) -> Vec<Device> {
        todo!()
    }

    /// The paired Macs, each with whether it answers on this network right now.
    pub fn near(&self, wait: Duration) -> Vec<Approver> {
        let _ = wait;
        todo!()
    }

    /// Broadcast and collect the Macs that answer within `wait`.
    pub fn find(&self, wait: Duration) -> Vec<Found> {
        let _ = wait;
        todo!()
    }

    /// Send `env` to a paired Mac on this network and wait for its answer (a person may be
    /// asked: up to two minutes). `not_reachable` if none answers the broadcast, `no_approver`
    /// with none paired.
    pub fn call(&self, env: Envelope) -> Reply {
        let _ = env;
        todo!()
    }

    /// Ask the Mac `fp` (found by [`Asker::find`]) to let this device in. Returns once its owner
    /// has answered, with the six digits; nothing is kept until [`Asker::confirm`].
    pub fn pair(&self, fp: &str) -> Result<PairAnswer, Refusal> {
        let _ = fp;
        todo!()
    }

    /// The wearer saw the same digits: keep the Mac in `approvers.json` (or, `ok` false, forget
    /// the attempt).
    pub fn confirm(&self, fp: &str, ok: bool) -> Result<(), Refusal> {
        let _ = (fp, ok);
        todo!()
    }
}
