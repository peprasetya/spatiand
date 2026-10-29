//! SpatiKey's service with a software stand-in for Touch ID, for testing what asks it (SpatiMesh,
//! the extensions, SpatiKey's window) end to end without a finger. **Never for real use**: the
//! vault key is kept in this process's memory and every "touch" says yes.
//!
//!   cargo run -p spatikey-core --example soft-service -- <folder> <socket>
//!
//! The vault is made in <folder>/apps/spatikey (and <folder>/local/spatikey) if it is not there;
//! every prompt's reason is printed, so a test can see what was asked.

use std::sync::{Arc, Mutex};

use spatikey_core::service::{Guard, Me, Mode, Service};
use spatikey_core::vault::{TouchId, VaultKey};
use spatikey_core::wire::{Code, Envelope, Refusal, Request, Who};

struct Yes {
    key: Mutex<Option<VaultKey>>,
}

impl Guard for Yes {
    fn unlock(&self, reason: &str, _touch: &TouchId, _linger_secs: u64) -> Result<VaultKey, Refusal> {
        println!("touch: {reason}");
        self.key.lock().unwrap().clone().ok_or_else(|| Refusal::new(Code::TouchIdUnavailable, "no key"))
    }

    fn enroll(&self, key: &VaultKey) -> Result<TouchId, Refusal> {
        *self.key.lock().unwrap() = Some(key.clone());
        Ok(TouchId { vault: String::new(), public: "c29mdA==".into(), blob: "c29mdA==".into(), wrapped: "c29mdA==".into() })
    }

    fn can_touch(&self) -> bool {
        true
    }

    fn allow_device(&self, name: &str, _fp: &str, code: &str) -> bool {
        println!("allow: {name} {code}");
        true
    }

    fn notice(&self, text: &str) {
        println!("notice: {text}");
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(folder), Some(socket)) = (args.next(), args.next()) else {
        eprintln!("usage: soft-service <folder> <socket>");
        std::process::exit(2);
    };
    let folder = std::path::PathBuf::from(folder);
    let (shared, local) = (folder.join("apps/spatikey"), folder.join("local/spatikey"));
    let guard = Arc::new(Yes { key: Mutex::new(None) });
    let me = Me { fp: "ab".repeat(32), name: "soft".into() };
    let service = Service::new(Mode::Approver(guard), me, shared, local);
    let setup = service.handle(1, Envelope { id: 1, who: Who::default(), request: Request::Setup { name: "soft".into() } });
    println!("setup: {}", if setup.err.is_none() { "made a vault" } else { "kept the vault there" });
    let _serving = spatikey_core::socket::serve(std::path::Path::new(&socket), service).expect("could not serve");
    println!("serving at {socket}");
    loop {
        std::thread::park();
    }
}
