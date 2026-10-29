//! The service: the only thing that reads the vault. docs/spatikey.md, "Asking for something".
//!
//! On a Mac with Touch ID it is an *approver*: it opens the vault with the [`Guard`] (the Swift
//! half) and answers. On the Deck and the Beam Pro it is a *forwarder*: it sends each request to
//! a Mac on the same network ([`crate::remote::Asker`]) and keeps for itself only what needs
//! this machine -- typing into its windows, the window to type into, finding and pairing Macs.
//!
//! STUB: the signatures are the contract; the bodies are to be written.

use std::path::PathBuf;
use std::sync::Arc;

use crate::vault::{TouchId, VaultKey};
use crate::wire::{Device, Envelope, Refusal, Reply};

/// What only the Mac's own code can do: the Secure Enclave and Touch ID, and asking its owner.
pub trait Guard: Send + Sync {
    /// Ask for a finger and hand back the vault key. `reason` finishes macOS's sentence
    /// "Spatiand is trying to ...". `linger_secs` lets one touch cover what follows for that
    /// long (0: this use only).
    fn unlock(&self, reason: &str, touch: &TouchId, linger_secs: u64) -> Result<VaultKey, Refusal>;
    /// Make this Mac's Secure Enclave key and wrap `key` to it. The answer's `vault` is left
    /// empty, for the caller to fill.
    fn enroll(&self, key: &VaultKey) -> Result<TouchId, Refusal>;
    /// Touch ID is there and has a finger enrolled (asks nothing).
    fn can_touch(&self) -> bool;
    /// The owner's yes or no to a device that wants to ask this Mac, with the six digits both show.
    fn allow_device(&self, name: &str, fp: &str, code: &str) -> bool;
    /// Tell the owner something (a card in the room, on the Mac's screen).
    fn notice(&self, text: &str);
}

/// This machine.
#[derive(Debug, Clone)]
pub struct Me {
    /// Its link fingerprint, 64 hex.
    pub fp: String,
    pub name: String,
}

/// One caller's connection: a session belongs to the connection that opened it.
pub type ConnId = u64;

/// A connection id never used before in this process.
pub fn next_conn() -> ConnId {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

pub enum Mode {
    Approver(Arc<dyn Guard>),
    Forwarder(Arc<crate::remote::Asker>),
}

pub struct Service {
    #[allow(dead_code)]
    mode: Mode,
    me: Me,
    #[allow(dead_code)]
    shared: PathBuf,
    #[allow(dead_code)]
    local: PathBuf,
}

impl Service {
    pub fn new(mode: Mode, me: Me, shared: PathBuf, local: PathBuf) -> Arc<Service> {
        Arc::new(Service { mode, me, shared, local })
    }

    pub fn me(&self) -> &Me {
        &self.me
    }

    pub fn is_approver(&self) -> bool {
        matches!(self.mode, Mode::Approver(_))
    }

    /// Answer one request from connection `conn`. Blocks while a person is asked.
    pub fn handle(&self, conn: ConnId, env: Envelope) -> Reply {
        let _ = (conn, env);
        todo!()
    }

    /// The connection has gone: its sessions close.
    pub fn closed(&self, conn: ConnId) {
        let _ = conn;
        todo!()
    }

    /// (Approver) the devices allowed to ask, from `local/spatikey/allowed.json`.
    pub fn allowed(&self) -> Vec<Device> {
        todo!()
    }

    /// (Approver) let a device ask from now on.
    pub fn allow(&self, device: Device) -> Result<(), String> {
        let _ = device;
        todo!()
    }

    /// (Approver) the Guard, for the remote side's pairing.
    pub fn guard(&self) -> Option<&Arc<dyn Guard>> {
        match &self.mode {
            Mode::Approver(guard) => Some(guard),
            Mode::Forwarder(_) => None,
        }
    }
}
