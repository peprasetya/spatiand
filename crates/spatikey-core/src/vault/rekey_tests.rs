//! A change of keys, with holders whose sensors are software P-256 keys and whose copies of the
//! synced folder meet as the synchroniser makes them meet.

use super::*;
use std::time::{Duration, UNIX_EPOCH};

/// Argon2id's least: quick enough for a debug build.
const TINY: (u32, u32, u32) = (8, 1, 1);

/// A device with a sensor, in software: its fingerprint, its P-256 key, and the public half.
struct Soft {
    fp: String,
    secret: p256::SecretKey,
    public: String,
}

impl Soft {
    fn new(n: u8) -> Soft {
        let secret = p256::SecretKey::random(&mut p256::elliptic_curve::rand_core::OsRng);
        let public = p256::elliptic_curve::sec1::ToEncodedPoint::to_encoded_point(&secret.public_key(), false).as_bytes().to_vec();
        Soft { fp: format!("{n:02x}").repeat(32), secret, public: b64::encode(&public) }
    }

    /// What its sensor does with a wrap: the ECDH, and the vault key out.
    fn open(&self, wrapped: &str) -> Result<VaultKey, String> {
        let wrapped = b64::decode(wrapped)?;
        let ephemeral = p256::PublicKey::from_sec1_bytes(wrapped.get(..holders::PUBLIC_LEN).ok_or("short")?).map_err(|e| e.to_string())?;
        let shared = p256::ecdh::diffie_hellman(self.secret.to_nonzero_scalar(), ephemeral.as_affine());
        holders::open_with_shared(shared.raw_secret_bytes(), &b64::decode(&self.public)?, &wrapped)
    }

    fn wrap(&self, key: &VaultKey) -> String {
        b64::encode(&holders::wrap(key, &b64::decode(&self.public).unwrap()).unwrap())
    }

    /// Its record as a holder adds it: sealed under `key`.
    fn record(&self, vault: &Vault, key: &VaultKey, name: &str) -> TouchDevice {
        let mut record = TouchDevice {
            device: self.fp.clone(),
            name: name.into(),
            kind: holders::MAC.into(),
            public: self.public.clone(),
            wrapped: self.wrap(key),
            added: crate::now_ms(),
            ..TouchDevice::default()
        };
        vault.seal_holder(key, &mut record).unwrap();
        record
    }
}

/// One device's copy of the synced folder and its own, and its sensor.
struct Device {
    soft: Soft,
    root: PathBuf,
}

impl Device {
    fn vault(&self) -> Vault {
        Vault::open(&self.root.join("apps"), &self.root.join("local")).unwrap()
    }
}

fn folder(name: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!("spatikey-rekey-{name}-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn files(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut walk = vec![dir.to_path_buf()];
    while let Some(dir) = walk.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => walk.push(entry.path()),
                _ => found.push(entry.path()),
            }
        }
    }
    found.sort();
    found
}

/// What the synchroniser does between two machines: of each file, the newer goes to the other,
/// with its time (`spatiand-sync`, "the newest version of a file wins").
fn sync(a: &Device, b: &Device) {
    let (a, b) = (a.root.join("apps"), b.root.join("apps"));
    for (from, to) in [(&a, &b), (&b, &a)] {
        for file in files(from) {
            let rel = file.strip_prefix(from).unwrap();
            if rel.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')) {
                continue;
            }
            let theirs = to.join(rel);
            let time = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).map(millis).ok();
            let bytes = std::fs::read(&file).unwrap();
            let newer = match (time(&file), time(&theirs)) {
                (Some(mine), Some(other)) => (mine, bytes.as_slice()) > (other, std::fs::read(&theirs).unwrap().as_slice()),
                (Some(_), None) => true,
                _ => false,
            };
            if newer {
                std::fs::create_dir_all(theirs.parent().unwrap()).unwrap();
                std::fs::write(&theirs, &bytes).unwrap();
                let at = UNIX_EPOCH + Duration::from_millis(time(&file).unwrap());
                std::fs::File::options().write(true).open(&theirs).unwrap().set_modified(at).unwrap();
            }
        }
    }
}

fn login(title: &str, password: &str) -> Entry {
    Entry { title: title.into(), username: "paulus".into(), password: password.into(), sites: vec![format!("{}.example", title.to_lowercase())], ..Entry::default() }
}

/// A vault three devices hold -- a Mac that made it, a phone, a tablet -- with three entries and
/// a recovery code, each device with its own copy of the synced folder, all the same.
fn three(name: &str) -> (Device, Device, Device, VaultKey, String) {
    let mut devices = (1..=3u8).map(|n| Device { soft: Soft::new(n), root: folder(&format!("{name}-{n}")) });
    let (mac, phone, tablet) = (devices.next().unwrap(), devices.next().unwrap(), devices.next().unwrap());
    let (vault, key) = Vault::create(&mac.root.join("apps"), &mac.root.join("local"), "01010101").unwrap();
    for (device, name) in [(&mac, "darkPhoenix"), (&phone, "Pixel"), (&tablet, "Tablet")] {
        vault.add_touch_device(&device.soft.record(&vault, &key, name)).unwrap();
    }
    for title in ["GitHub", "Bank", "Steam"] {
        vault.write(&key, &mut login(title, &format!("{title}-secret")), "01010101").unwrap();
    }
    let code = vault.set_recovery_with(&key, TINY).unwrap();
    sync(&mac, &phone);
    sync(&mac, &tablet);
    for device in [&mac, &phone, &tablet] {
        let vault = device.vault();
        let touch = TouchId { vault: String::new(), public: device.soft.public.clone(), blob: "YmxvYg==".into(), wrapped: device.soft.wrap(&key) };
        vault.set_touch_id(&touch).unwrap();
        assert_eq!(vault.settle(&key, &device.soft.fp).unwrap(), 0);
    }
    (mac, phone, tablet, key, code)
}

/// What a holder does when it next unlocks: the one change that follows from its key, its wrap
/// opened by its sensor, believed if it leads back to the key it had.
fn follow(device: &Device) -> Result<VaultKey, String> {
    let vault = device.vault();
    let half = vault.half().ok_or("no half")?;
    let after = vault.successors(&half.key, &device.soft.fp, &half.touch.public);
    let real: Vec<VaultKey> = after
        .iter()
        .filter_map(|change| device.soft.open(&change.wrapped).ok().filter(|key| vault.follows(key, &change.id, &half.key, &half.proof)))
        .collect();
    let key = match real.len() {
        0 => {
            let key = device.soft.open(&half.touch.wrapped)?;
            if !vault.follows(&key, &half.key, &half.key, &half.proof) {
                return Err("its own wrap gives a key that is not the vault's".into());
            }
            key
        }
        1 => real.into_iter().next().unwrap(),
        _ => return Err(RIVALS.into()),
    };
    vault.settle(&key, &device.soft.fp)?;
    Ok(key)
}

fn titles(vault: &Vault, key: &VaultKey) -> Vec<(String, String)> {
    vault.entries(key).unwrap().into_iter().map(|e| (e.title, e.password)).collect()
}

fn all_three() -> Vec<(String, String)> {
    vec![("Bank".into(), "Bank-secret".into()), ("GitHub".into(), "GitHub-secret".into()), ("Steam".into(), "Steam-secret".into())]
}

const BY: &str = "01010101";

#[test]
fn removing_a_holder_makes_a_key_the_removed_one_never_had() {
    let (mac, phone, tablet, old, code) = three("remove");
    let vault = mac.vault();
    let before: Vec<(Entry, u64)> = vault
        .entries(&old)
        .unwrap()
        .into_iter()
        .map(|e| {
            let at = millis(std::fs::metadata(vault.entry_path(&e.id)).unwrap().modified().unwrap());
            (e, at)
        })
        .collect();
    // What the tablet could have kept: the old key, its own key, its wrap.
    let kept_wrap = tablet.vault().half().unwrap().touch.wrapped;
    assert_eq!(tablet.soft.open(&kept_wrap).unwrap().bytes(), old.bytes());

    let made = vault.rekey(&old, &[tablet.soft.fp.clone()], &mac.soft.fp, BY, RecoveryPlan::New, TINY).unwrap();
    assert!(made.recovery_code.is_none(), "the code is wrapped to again, not replaced");
    assert!(made.left_out.is_empty());
    assert_ne!(made.key.bytes(), old.bytes());
    assert_eq!(vault.settle(&made.key, &mac.soft.fp).unwrap(), 3, "every entry sealed again");
    assert_eq!(vault.settle(&made.key, &mac.soft.fp).unwrap(), 0, "and once is enough");

    // The vault as it now is: everything, to the new key, and each entry what it was.
    assert_eq!(titles(&vault, &made.key), all_three());
    for (was, at) in &before {
        let now = vault.read(&made.key, &was.id).unwrap();
        assert_eq!((now.version, now.changed, &now.changed_by, &now.password), (was.version, was.changed, &was.changed_by, &was.password));
        let moved = millis(std::fs::metadata(vault.entry_path(&was.id)).unwrap().modified().unwrap());
        assert_eq!(moved, at + 1, "sealed again a millisecond after, not now");
    }
    // The old key reads none of it: it is told the keys changed, and no file opens with it.
    assert_eq!(vault.entries(&old).unwrap_err(), REKEYED);
    let old_keys = Keys::new(vault.id(), "", &old).unwrap();
    for (entry, _) in &before {
        let bytes = std::fs::read(vault.entry_path(&entry.id)).unwrap();
        assert!(open_entry(&old_keys, &entry.id, &bytes).is_err(), "the old key opened an entry sealed again");
        assert_eq!(vault.read(&old, &entry.id).unwrap_err(), REKEYED);
    }
    assert!(vault.write(&old, &mut before[0].0.clone(), "03030303").is_err(), "nor does it write over one");
    // Nothing synced is wrapped to the tablet any more, and its record is a tombstone.
    assert!(vault.touch_device(&tablet.soft.fp).is_some_and(|d| d.removed && d.wrapped.is_empty() && d.public.is_empty()));
    for file in files(&mac.root.join("apps")) {
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(!text.contains(&tablet.soft.public) && !text.contains(&kept_wrap), "{} still names the tablet's key", file.display());
        assert!(!text.contains(&b64::encode(made.key.bytes())) && !text.contains(&b64::encode(old.bytes())), "{} holds a key in the clear", file.display());
    }
    // The tablet, with everything it kept and the vault as it now is: its wrap gives the old key,
    // no change of keys has a wrap for it, and its key opens nobody else's.
    sync(&mac, &tablet);
    let theirs = tablet.vault();
    assert!(theirs.successors("", &tablet.soft.fp, &tablet.soft.public).is_empty());
    assert_eq!(theirs.generation_of(&old).as_deref(), Some(""));
    assert_eq!(theirs.entries(&old).unwrap_err(), REKEYED);
    for holder in &theirs.rekey_record(&made.id).unwrap().holders {
        assert!(tablet.soft.open(&holder.wrapped).is_err(), "the tablet's key opened another holder's wrap");
    }
    assert!(theirs.left_out("", &tablet.soft.fp, &tablet.soft.public));

    // The phone was away: it has the new key at its next unlock, by its own sensor alone.
    sync(&mac, &phone);
    let theirs = phone.vault();
    assert_eq!(theirs.half().unwrap().key, "", "still at the first key");
    let followed = follow(&phone).unwrap();
    assert_eq!(followed.bytes(), made.key.bytes());
    assert_eq!(theirs.half().unwrap().key, made.id);
    assert_eq!(titles(&theirs, &followed), all_three());
    assert_eq!(follow(&phone).unwrap().bytes(), made.key.bytes(), "and goes on having it");
    // Its record and the Mac's say the new key, sealed under it.
    for device in [&mac, &phone] {
        let record = vault.touch_device(&device.soft.fp).unwrap();
        assert!(record.key == made.id && vault.holder_sealed(&made.key, &record) && !vault.holder_sealed(&old, &record));
        assert_eq!(device.soft.open(&record.wrapped).unwrap().bytes(), made.key.bytes());
    }

    // The recovery code is the same code, and gives the new key.
    assert_eq!(vault.recover(&code).unwrap().bytes(), made.key.bytes());
    assert_eq!(vault.recovery_file().unwrap().key, made.id);
    assert!(vault.recover(&recovery::new_code()).is_err());
}

#[test]
fn a_change_of_keys_broken_off_is_finished_by_whoever_opens_the_vault_next() {
    let (mac, _phone, tablet, old, code) = three("broken");
    let vault = mac.vault();
    // Before the one file is written there is nothing: a key made and thrown away.
    assert!(vault.rekeys().is_empty());
    // Broken off right after it: no entry sealed again, no tombstone, no record changed,
    // recovery.json the old one, this holder's own place the old one.
    let made = vault.rekey(&old, &[tablet.soft.fp.clone()], &mac.soft.fp, BY, RecoveryPlan::New, TINY).unwrap();
    assert_eq!(vault.half().unwrap().key, "");
    assert!(!vault.touch_device(&tablet.soft.fp).unwrap().removed);
    assert_eq!(vault.recovery_file().unwrap().key, "");
    // The vault is whole all the same: the new key reads every entry, through the old one it
    // carries; the recovery code gives the new key.
    assert_eq!(titles(&vault, &made.key), all_three());
    assert_eq!(vault.recover(&code).unwrap().bytes(), made.key.bytes());
    // The Mac comes back and unlocks: it follows its own change, and finishes it.
    let followed = follow(&mac).unwrap();
    assert_eq!(followed.bytes(), made.key.bytes());
    assert_eq!(vault.half().unwrap().key, made.id);
    assert!(vault.touch_device(&tablet.soft.fp).unwrap().removed);
    assert_eq!(vault.recovery_file().unwrap().key, made.id);
    assert_eq!(vault.entries(&old).unwrap_err(), REKEYED);
    assert_eq!(titles(&vault, &made.key), all_three());

    // Or the phone does, if the Mac never does: the same end.
    let (mac, phone, tablet, old, _) = three("broken-other");
    let made = mac.vault().rekey(&old, &[tablet.soft.fp.clone()], &mac.soft.fp, BY, RecoveryPlan::New, TINY).unwrap();
    sync(&mac, &phone);
    let followed = follow(&phone).unwrap();
    assert_eq!(followed.bytes(), made.key.bytes());
    let theirs = phone.vault();
    assert!(theirs.touch_device(&tablet.soft.fp).unwrap().removed);
    assert_eq!(theirs.entries(&old).unwrap_err(), REKEYED);
    assert_eq!(titles(&theirs, &followed), all_three());
}

#[test]
fn an_entry_changed_elsewhere_while_the_keys_changed_is_not_lost() {
    let (mac, phone, tablet, old, _) = three("meanwhile");
    // The phone, out of touch, changes one entry and adds another, under the key it has.
    std::thread::sleep(Duration::from_millis(5));
    let theirs = phone.vault();
    let mut bank = theirs.entries(&old).unwrap().into_iter().find(|e| e.title == "Bank").unwrap();
    bank.password = "Bank-changed".into();
    theirs.write(&old, &mut bank, "02020202").unwrap();
    theirs.write(&old, &mut login("Forum", "Forum-secret"), "02020202").unwrap();
    // Meanwhile the Mac removes the tablet. Its sealing again is a millisecond after each entry's
    // own time: older than the phone's change, though it was done later.
    std::thread::sleep(Duration::from_millis(5));
    let vault = mac.vault();
    let made = vault.rekey(&old, &[tablet.soft.fp.clone()], &mac.soft.fp, BY, RecoveryPlan::New, TINY).unwrap();
    vault.settle(&made.key, &mac.soft.fp).unwrap();
    // They meet. The phone's two files are the newer ones and win, sealed under the old key.
    sync(&mac, &phone);
    let expected: Vec<(String, String)> = vec![
        ("Bank".into(), "Bank-changed".into()),
        ("Forum".into(), "Forum-secret".into()),
        ("GitHub".into(), "GitHub-secret".into()),
        ("Steam".into(), "Steam-secret".into()),
    ];
    // The Mac reads them at once -- the new key carries the old -- and seals them again.
    assert_eq!(titles(&vault, &made.key), expected);
    assert_eq!(vault.settle(&made.key, &mac.soft.fp).unwrap(), 2);
    assert_eq!(vault.read(&made.key, &bank.id).unwrap().version, bank.version, "the phone's change as it made it");
    // The phone follows, and both ends are the same.
    let followed = follow(&phone).unwrap();
    sync(&mac, &phone);
    assert_eq!(titles(&phone.vault(), &followed), expected);
    assert_eq!(titles(&vault, &made.key), expected);
    assert_eq!(vault.entries(&old).unwrap_err(), REKEYED);
    for id in vault.entry_ids().unwrap() {
        assert_eq!(std::fs::read(vault.entry_path(&id)).unwrap(), std::fs::read(phone.vault().entry_path(&id)).unwrap());
    }
}

#[test]
fn a_holder_follows_only_a_change_that_leads_back_to_the_key_it_had() {
    let (mac, phone, tablet, old, _) = three("rival");
    let vault = mac.vault();
    let made = vault.rekey(&old, &[tablet.soft.fp.clone()], &mac.soft.fp, BY, RecoveryPlan::New, TINY).unwrap();
    vault.settle(&made.key, &mac.soft.fp).unwrap();

    // Somebody who can write the synced folder and has no key leaves a change of its own, with a
    // wrap for the phone: the key in it leads nowhere.
    let forged = VaultKey::random();
    let keys = Keys::new(vault.id(), "aaaaaaaaaaaaaaaa", &forged).unwrap();
    let mut fake = Rekey {
        v: 1,
        id: "aaaaaaaaaaaaaaaa".into(),
        prev: made.id.clone(),
        made: crate::now_ms(),
        by: "0f0f0f0f".into(),
        check: keys.seal_check().unwrap(),
        before: b64::encode(&[0u8; 60]),
        holders: vec![RekeyHolder { device: phone.soft.fp.clone(), public: phone.soft.public.clone(), wrapped: phone.soft.wrap(&forged) }],
        ..Rekey::default()
    };
    fake.seal = keys.sign(&rekey_text(vault.id(), &fake));
    write_json(&vault.chain_path(&fake.id), &fake).unwrap();
    sync(&mac, &phone);
    let theirs = phone.vault();
    assert_eq!(theirs.successors("", &phone.soft.fp, &phone.soft.public).len(), 1, "the last of the line is the forgery");
    assert!(!theirs.follows(&forged, &fake.id, "", &theirs.half().unwrap().proof));
    // The Mac, at the real key, is not moved either: the forgery does not follow from it.
    assert!(!vault.follows(&forged, &fake.id, &made.id, &vault.half().unwrap().proof));
    std::fs::remove_file(vault.chain_path(&fake.id)).unwrap();
    std::fs::remove_file(theirs.chain_path(&fake.id)).unwrap();

    // The removed tablet, which had the old key, makes a change of its own from it, leaving the
    // Mac out. To the phone, still at the old key, both lead back to the key it has.
    let rival = tablet.vault().rekey(&old, &[mac.soft.fp.clone()], &tablet.soft.fp, "03030303", RecoveryPlan::New, TINY).unwrap();
    sync(&tablet, &phone);
    let theirs = phone.vault();
    let half = theirs.half().unwrap();
    assert_eq!(theirs.successors("", &phone.soft.fp, &phone.soft.public).len(), 2);
    assert!(theirs.follows(&made.key, &made.id, "", &half.proof) && theirs.follows(&rival.key, &rival.id, "", &half.proof));
    assert_eq!(follow(&phone).unwrap_err(), RIVALS, "it follows neither");
    // The Mac had moved on before the rival was made: it is at its own key and stays there.
    sync(&tablet, &mac);
    assert!(vault.successors(&made.id, &mac.soft.fp, &mac.soft.public).is_empty());
    assert_eq!(follow(&mac).unwrap().bytes(), made.key.bytes());
    assert!(!vault.follows(&rival.key, &rival.id, &made.id, &vault.half().unwrap().proof));
    // A proof kept of another key than the one a change leads back to is not satisfied.
    assert!(!theirs.follows(&made.key, &made.id, "", &vault.half().unwrap().proof));
}

#[test]
fn a_new_key_is_wrapped_only_to_holders_a_holder_sealed() {
    let (mac, phone, tablet, old, _) = three("sealed");
    let vault = mac.vault();
    // Left in the synced folder by something that can write it and has no key: a record with a
    // key of its own, and one that copies a real holder's seal onto another key.
    let planted = Soft::new(9);
    let mut record = TouchDevice {
        device: planted.fp.clone(),
        name: "Not a holder".into(),
        kind: holders::MAC.into(),
        public: planted.public.clone(),
        wrapped: planted.wrap(&VaultKey::random()),
        added: 1,
        ..TouchDevice::default()
    };
    vault.add_touch_device(&record).unwrap();
    let copied = Soft::new(8);
    let real = vault.touch_device(&phone.soft.fp).unwrap();
    vault.add_touch_device(&TouchDevice { device: copied.fp.clone(), public: copied.public.clone(), name: "A copy".into(), ..real.clone() }).unwrap();
    assert!(!vault.holder_sealed(&old, &vault.touch_device(&copied.fp).unwrap()));

    let made = vault.rekey(&old, &[tablet.soft.fp.clone()], &mac.soft.fp, BY, RecoveryPlan::New, TINY).unwrap();
    let mut left_out = made.left_out.clone();
    left_out.sort();
    assert_eq!(left_out, ["A copy", "Not a holder"]);
    let change = vault.rekey_record(&made.id).unwrap();
    let wrapped_to: Vec<&str> = change.holders.iter().map(|h| h.device.as_str()).collect();
    assert_eq!(wrapped_to, [mac.soft.fp.as_str(), phone.soft.fp.as_str()]);
    vault.settle(&made.key, &mac.soft.fp).unwrap();

    // Sealed under the old key since -- by the tablet, which still has it: not a holder of the
    // new one, and not wrapped to at the next change.
    vault.seal_holder(&old, &mut record).unwrap();
    vault.add_touch_device(&record).unwrap();
    assert!(vault.holder_sealed(&old, &record) && !vault.holder_sealed(&made.key, &record));
    let next = vault.rekey(&made.key, &[copied.fp.clone()], &mac.soft.fp, BY, RecoveryPlan::New, TINY).unwrap();
    assert!(next.left_out.contains(&"Not a holder".to_string()));
    assert!(vault.rekey_record(&next.id).unwrap().holders.iter().all(|h| h.device != planted.fp));
    // A change altered after it was made -- a wrap added for somebody -- is not the key's.
    let mut altered = vault.rekey_record(&next.id).unwrap();
    altered.holders.push(RekeyHolder { device: planted.fp.clone(), public: planted.public.clone(), wrapped: planted.wrap(&next.key) });
    write_json(&vault.chain_path(&next.id), &altered).unwrap();
    assert!(vault.generation_of(&next.key).is_none() && vault.ring(&next.key).is_err());
}

#[test]
fn a_recovery_code_from_before_is_kept_by_being_typed_or_replaced() {
    for typed in [true, false] {
        let (mac, _phone, tablet, old, _) = three(if typed { "typed" } else { "replaced" });
        let vault = mac.vault();
        // As vaults made before 2026-11-18 have it: Argon2id's key wrapping the vault key.
        let code = recovery::new_code();
        write_json(&vault.shared.join("recovery.json"), &recovery::wrap_with(&old, &code, vault.id(), TINY).unwrap()).unwrap();
        assert_eq!(vault.recover(&code).unwrap().bytes(), old.bytes());
        let removed = [tablet.soft.fp.clone()];
        if typed {
            let wrong = recovery::new_code();
            assert!(vault.rekey(&old, &removed, &mac.soft.fp, BY, RecoveryPlan::Keep(&wrong), TINY).is_err());
            assert!(vault.rekeys().is_empty(), "a wrong code changes nothing");
        }
        let plan = if typed { RecoveryPlan::Keep(&code) } else { RecoveryPlan::New };
        let made = vault.rekey(&old, &removed, &mac.soft.fp, BY, plan, TINY).unwrap();
        vault.settle(&made.key, &mac.soft.fp).unwrap();
        let record = vault.recovery_file().unwrap();
        assert!(record.rewraps() && record.key == made.id, "the record is of the kind that needs no code next time");
        match &made.recovery_code {
            None => {
                assert!(typed);
                assert_eq!(vault.recover(&code).unwrap().bytes(), made.key.bytes());
            }
            Some(new) => {
                assert!(!typed);
                assert_eq!(vault.recover(new).unwrap().bytes(), made.key.bytes());
                assert!(vault.recover(&code).is_err(), "the code before opens the vault no more");
            }
        }
        // And the next change needs neither.
        let code_now = made.recovery_code.as_ref().map(|c| c.to_string()).unwrap_or(code);
        let next = vault.rekey(&made.key, &[], &mac.soft.fp, BY, RecoveryPlan::New, TINY).unwrap();
        assert!(next.recovery_code.is_none());
        assert_eq!(vault.recover(&code_now).unwrap().bytes(), next.key.bytes());
    }
}

#[test]
fn keys_changed_twice_are_followed_in_one_step_and_read_all_the_way_back() {
    let (mac, phone, tablet, old, code) = three("twice");
    let vault = mac.vault();
    let first = vault.rekey(&old, &[tablet.soft.fp.clone()], &mac.soft.fp, BY, RecoveryPlan::New, TINY).unwrap();
    // Not settled: an entry written under the first new key, then the keys changed again.
    vault.write(&first.key, &mut login("Forum", "Forum-secret"), BY).unwrap();
    let second = vault.rekey(&first.key, &[], &mac.soft.fp, BY, RecoveryPlan::New, TINY).unwrap();
    assert_eq!(vault.rekey(&first.key, &[], &mac.soft.fp, BY, RecoveryPlan::New, TINY).err().as_deref(), Some(REKEYED), "only the newest key makes the next");
    sync(&mac, &phone);
    let followed = follow(&phone).unwrap();
    assert_eq!(followed.bytes(), second.key.bytes());
    let theirs = phone.vault();
    assert_eq!(theirs.half().unwrap().key, second.id);
    assert_eq!(titles(&theirs, &followed).len(), 4, "entries of three keys' sealing, all read");
    assert_eq!(theirs.settle(&followed, &phone.soft.fp).unwrap(), 0, "and all sealed under the newest by then");
    assert_eq!(theirs.recover(&code).unwrap().bytes(), second.key.bytes());
    assert_eq!(theirs.entries(&first.key).unwrap_err(), REKEYED);
    assert_eq!(theirs.entries(&old).unwrap_err(), REKEYED);
}
