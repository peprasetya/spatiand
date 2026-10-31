//! Whether this device reaches a Mac's SpatiKey, as a device paired with it as a host does, with no
//! pairing of SpatiKey's own. Asks only for the status, which raises no prompt on the Mac.
//!
//!   cargo run -p spatikey-core --example ask-mac -- <identity folder> <the Mac's fingerprint>

use std::path::PathBuf;
use std::time::Duration;

use spatikey_core::remote::Asker;
use spatikey_core::wire::{Device, Envelope, Request, Who};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (Some(dir), Some(fp)) = (args.get(1), args.get(2).cloned()) else {
        eprintln!("usage: ask-mac <identity folder> <the Mac's fingerprint>");
        std::process::exit(2);
    };
    let identity = spatiand_stream::Identity::load_or_create(&PathBuf::from(dir)).expect("an identity");
    println!("this device is {}", identity.fingerprint());
    let local = std::env::temp_dir().join(format!("ask-mac-{}", std::process::id()));
    let asker = Asker::new(identity, local, "ask-mac".into());
    asker.also_ask(Box::new(move || vec![Device { fp: fp.clone(), name: "the host".into(), added: 0 }]));
    println!("Macs near: {:?}", asker.near(Duration::from_secs(2)));
    let reply = asker.call(Envelope { id: 1, who: Who::default(), request: Request::Status });
    println!("status: {reply:?}");
}
