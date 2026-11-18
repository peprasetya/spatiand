//! SpatiKey's service with a software stand-in for a holder's sensor, for testing what asks it
//! (SpatiMesh, the extensions, SpatiKey's window) end to end without a finger. **Never for real
//! use**: the holder's key is a P-256 key kept in a file in the clear, and every "touch" says yes.
//!
//!   cargo run -p spatikey-core --example soft-service -- <folder> <socket> [<name> [android]]
//!
//! The vault is made in <folder>/apps/spatikey if it is not there; this holder's own files are in
//! <folder>/local/spatikey, or <folder>/local-<name>/spatikey when it is given a name. A second one
//! with another name and socket over the same folder is a second device with biometrics (an
//! Android phone with `android`): it can ask to be added (`holder_request`), the first add it
//! (`holder_approve`), and both open the same vault. Every prompt's reason is printed, so a test
//! can see what was asked.
//!
//! `SOFT_BREAK_REKEY=<name>` is a holder that dies in the middle of removing the holder of that
//! name: it makes the vault its new keys -- the one file that is the change -- and stops there,
//! with no entry sealed again, no tombstone and its own half as it was, as a crash at the worst
//! moment leaves things. Started again without it, it finishes. `tools/spatikey-rekey-rig.py`
//! drives three of these over one folder.

use std::sync::Arc;

use p256::elliptic_curve::sec1::ToEncodedPoint;
use spatikey_core::service::{Guard, Me, Mode, Service};
use spatikey_core::vault::{TouchId, VaultKey};
use spatikey_core::wire::{Code, Envelope, Refusal, Request, Who};
use spatikey_core::{b64, holders};

/// A holder whose key is in software: the ECDH a Secure Enclave or a Keystore does, with no finger.
struct Yes {
    kind: &'static str,
}

impl Guard for Yes {
    fn unlock(&self, reason: &str, touch: &TouchId, _linger_secs: u64) -> Result<VaultKey, Refusal> {
        println!("touch: {reason}");
        let failed = |e: String| Refusal::new(Code::FingerprintsChanged, e);
        let secret = p256::SecretKey::from_slice(&b64::decode(&touch.blob).map_err(failed)?).map_err(|e| failed(e.to_string()))?;
        let wrapped = b64::decode(&touch.wrapped).map_err(failed)?;
        if wrapped.len() != holders::WRAPPED_LEN {
            return Err(failed("not a wrap".into()));
        }
        let ephemeral = p256::PublicKey::from_sec1_bytes(&wrapped[..holders::PUBLIC_LEN]).map_err(|e| failed(e.to_string()))?;
        let shared = p256::ecdh::diffie_hellman(secret.to_nonzero_scalar(), ephemeral.as_affine());
        holders::open_with_shared(shared.raw_secret_bytes(), &b64::decode(&touch.public).map_err(failed)?, &wrapped).map_err(failed)
    }

    fn enroll(&self, key: &VaultKey) -> Result<TouchId, Refusal> {
        let mut made = self.make()?;
        let public = b64::decode(&made.public).map_err(|e| Refusal::new(Code::Failed, e))?;
        made.wrapped = b64::encode(&holders::wrap(key, &public).map_err(|e| Refusal::new(Code::Failed, e))?);
        Ok(made)
    }

    fn make(&self) -> Result<TouchId, Refusal> {
        let secret = p256::SecretKey::random(&mut p256::elliptic_curve::rand_core::OsRng);
        let public = secret.public_key().to_encoded_point(false).as_bytes().to_vec();
        Ok(TouchId { vault: String::new(), public: b64::encode(&public), blob: b64::encode(&secret.to_bytes()), wrapped: String::new() })
    }

    fn kind(&self) -> &'static str {
        self.kind
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
        eprintln!("usage: soft-service <folder> <socket> [<name> [android]]");
        std::process::exit(2);
    };
    let name = args.next();
    let kind = if args.next().as_deref() == Some("android") { holders::ANDROID } else { holders::MAC };
    let folder = std::path::PathBuf::from(folder);
    let shared = folder.join("apps/spatikey");
    let local = match &name {
        Some(name) => folder.join(format!("local-{name}/spatikey")),
        None => folder.join("local/spatikey"),
    };
    // A fingerprint of its own for each name, as each device has.
    let fp = match &name {
        Some(name) => ring::digest::digest(&ring::digest::SHA256, name.as_bytes()).as_ref().iter().map(|b| format!("{b:02x}")).collect(),
        None => "ab".repeat(32),
    };
    let name = name.unwrap_or_else(|| "soft".into());
    if let Ok(removed) = std::env::var("SOFT_BREAK_REKEY") {
        let removed: String = ring::digest::digest(&ring::digest::SHA256, removed.as_bytes()).as_ref().iter().map(|b| format!("{b:02x}")).collect();
        let vault = spatikey_core::vault::Vault::open(&shared, &local).expect("a vault");
        let half = vault.half().expect("this holder's half");
        let key = Yes { kind }.unlock("remove a holder, and die", &half.touch, 0).expect("the vault key");
        let plan = spatikey_core::vault::RecoveryPlan::New;
        let made = vault.rekey(&key, &[removed], &fp, &fp[..8], plan, spatikey_core::recovery::COST).expect("new keys");
        println!("new keys {} made; stopping before anything else is done", made.id);
        std::process::exit(9);
    }
    let me = Me { fp: fp.clone(), name: name.clone() };
    let service = Service::new(Mode::Approver(Arc::new(Yes { kind })), me, shared, local);
    let setup = service.handle(1, Envelope { id: 1, who: Who::default(), request: Request::Setup { name: name.clone() } });
    println!("setup: {}", if setup.err.is_none() { "made a vault" } else { "kept the vault there" });
    let _serving = spatikey_core::socket::serve(std::path::Path::new(&socket), service).expect("could not serve");
    println!("serving at {socket} as {name} ({})", &fp[..8]);
    loop {
        std::thread::park();
    }
}
