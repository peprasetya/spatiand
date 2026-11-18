//! A new vault key, when a holder is removed. docs/spatikey.md, "Removing a holder makes new keys".
//!
//! A removed device once had the vault key. So removing it makes a new one: every entry is sealed
//! again under it, and it is wrapped to each holder that stays and to the recovery code -- to their
//! *public* keys, so none of them is asked for anything.
//!
//! **One file is the change.** `chain/<id>.json` ([`Rekey`]) holds the new key wrapped to every
//! remaining holder and to the recovery code, the key before it sealed under the new one, who was
//! removed, and a seal over all of it that only the new key makes. It is written once, whole (a
//! hidden file, then a rename): before it there is the old vault, after it the new one, and
//! everything else -- the entries sealed again, the removed device's tombstone, the holders'
//! records, `recovery.json` -- is done after it and done again by whoever opens the vault next
//! ([`Vault::settle`]) if it was not finished. The folder is named to come before `devices` and
//! `entries` in the synchroniser's order, which sends a folder's files by name: a device has the
//! change of keys before it has an entry sealed under them.
//!
//! **Old keys are kept, sealed under the new.** The synchroniser keeps the newest *file*, so an
//! entry changed on a device that had not yet heard of the new key arrives sealed under the old
//! one, newer than the copy sealed again here. Each change of keys carries the key before it,
//! sealed under its own: a holder with the newest key reads back through all of them, and seals
//! such an entry again when it next opens the vault. Going the other way is what must not be
//! possible, and is not: nothing sealed under an old key gives a newer one. An entry sealed again
//! keeps its place in time (its file's time moves one millisecond, not to now), so that sealing
//! does not outrun a real change made elsewhere at about the same time.
//!
//! **Following.** A holder knows which key it is at (`local/spatikey/follow.json`: the change's
//! name, its wrap of that key, and a proof only that key makes). When it next unlocks it looks for
//! changes that follow from its own, takes the newest's wrap to its sensor -- one touch, as ever --
//! and believes the key that comes out only if the chain of sealed keys leads from it back to the
//! key it had ([`Vault::follows`]). Two changes that each follow from its key and each check out
//! are two devices that each removed somebody while out of touch, or a removed device passing
//! itself off: it follows neither.

use std::collections::HashSet;

use ring::hmac;
use serde::{Deserialize, Serialize};

use super::*;
use crate::holders;
use crate::recovery::{self, RecoveryRecord};

/// What a session is told when the vault's keys were changed under it.
pub const REKEYED: &str = "the vault's keys were changed on another device: open SpatiKey again";

/// What a device is told when two changes of keys each follow from the one it has.
pub const RIVALS: &str = "two devices have each made new keys for this vault: this device follows neither. Add it again from the one you trust";

/// What the holder-check proof and the seals are made over, besides what they say.
const PROOF: &[u8] = b"spatikey holder check v1";
const HOLDER: &str = "spatikey holder v1";
const REKEY: &str = "spatikey rekey v1";

/// The most changes of keys walked through: far more than a vault ever has, and an end to a
/// chain somebody tied into a ring.
const LONGEST_CHAIN: usize = 4096;

/// `chain/<id>.json`: one change of keys.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Rekey {
    pub v: u32,
    /// 16 hex characters, random: the name of the key this change made.
    pub id: String,
    /// The key before it: another change's name, or empty for the vault's first key.
    #[serde(default)]
    pub prev: String,
    pub made: u64,
    /// The short fingerprint of the holder that made it.
    pub by: String,
    /// The devices removed with it, by fingerprint.
    #[serde(default)]
    pub removed: Vec<String>,
    /// "spatikey" sealed under the new key's entry key: tells the right key from a wrong one.
    pub check: String,
    /// The key before, sealed under the new one (base64 of nonce ‖ ciphertext ‖ tag).
    pub before: String,
    /// The new key wrapped to each holder that stays.
    pub holders: Vec<RekeyHolder>,
    /// The new key wrapped to the recovery code's key, with what the code needs to make that key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery: Option<RecoveryRecord>,
    /// HMAC-SHA256 over all of the above, under a key made from the new vault key.
    pub seal: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RekeyHolder {
    /// The holder's link fingerprint, 64 hex.
    pub device: String,
    /// Its key's public half, X9.63, base64.
    pub public: String,
    /// The new vault key wrapped to it.
    pub wrapped: String,
}

/// `local/spatikey/follow.json`: which of the vault's keys this holder is at.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Follow {
    pub vault: String,
    /// The holder's own public key: a half made anew (the recovery code) is not this one's.
    pub public: String,
    /// The change of keys that made the key, or empty for the vault's first.
    #[serde(default)]
    pub key: String,
    /// That key wrapped to this holder.
    pub wrapped: String,
    /// What only that key makes, kept where nothing synced can change it: the key a later change
    /// claims to follow from is checked against this. Empty until the key has been had once.
    #[serde(default)]
    pub proof: String,
}

/// This device's half as it is used: its key's blob with the wrap of the key it is at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Half {
    pub touch: TouchId,
    /// The change of keys it is at; empty for the vault's first key.
    pub key: String,
    pub proof: String,
}

/// A change of keys that follows from the one a holder is at, with its wrap for that holder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Successor {
    pub id: String,
    pub wrapped: String,
}

/// What a removal does about the recovery code when the vault's record of it is of version 1,
/// which only the code can wrap a new key to.
pub enum RecoveryPlan<'a> {
    /// The owner typed the code: it goes on opening the vault.
    Keep(&'a str),
    /// A new code is made, to be shown once; the old one opens only what was sealed before.
    New,
}

/// A change of keys, made.
pub struct Rekeyed {
    pub key: VaultKey,
    pub id: String,
    /// A new recovery code, where one had to be made.
    pub recovery_code: Option<Zeroizing<String>>,
    /// Holders the new key was not wrapped to, by name: their records were not sealed by a holder
    /// under the key before (written before 2026-11-18, or by something that is not a holder).
    /// Each is added again as a new device is.
    pub left_out: Vec<String>,
}

/// The vault's keys as one holder has them: the one it is at first, then each before it.
pub(crate) struct Ring {
    keys: Vec<Keys>,
    raw: Vec<VaultKey>,
}

impl Ring {
    pub(crate) fn head(&self) -> &Keys {
        &self.keys[0]
    }

    pub(crate) fn by(&self, id: &str) -> Option<&Keys> {
        self.keys.iter().find(|keys| keys.id == id)
    }
}

fn is_rekey_id(id: &str) -> bool {
    id.len() == 16 && id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Parts, each with its length before it: no two lists of parts give the same bytes.
fn framed(parts: &[&str]) -> Vec<u8> {
    let mut text = Vec::new();
    for part in parts {
        text.extend_from_slice(&(part.len() as u32).to_be_bytes());
        text.extend_from_slice(part.as_bytes());
    }
    text
}

/// What a change's seal is made over: everything in it but the seal.
fn rekey_text(vault: &str, record: &Rekey) -> Vec<u8> {
    let made = record.made.to_string();
    let count = record.holders.len().to_string();
    let removed = record.removed.len().to_string();
    let mut parts: Vec<&str> = vec![REKEY, vault, &record.id, &record.prev, &made, &record.by, &removed];
    parts.extend(record.removed.iter().map(String::as_str));
    parts.extend([record.check.as_str(), record.before.as_str(), count.as_str()]);
    for holder in &record.holders {
        parts.extend([holder.device.as_str(), holder.public.as_str(), holder.wrapped.as_str()]);
    }
    let cost;
    match &record.recovery {
        Some(r) => {
            cost = format!("{}/{}/{}/{}/{}", r.v, r.kdf, r.m, r.t, r.p);
            parts.extend(["recovery", cost.as_str(), r.salt.as_str(), r.public.as_str(), r.wrapped.as_str(), r.key.as_str()]);
        }
        None => parts.push("no recovery"),
    }
    framed(&parts)
}

fn holder_text(vault: &str, key: &str, device: &TouchDevice) -> Vec<u8> {
    framed(&[HOLDER, vault, key, &device.device.to_ascii_lowercase(), &device.kind, &device.public])
}

impl Keys {
    fn sign(&self, text: &[u8]) -> String {
        b64::encode(hmac::sign(&self.seals, text).as_ref())
    }

    fn signed(&self, text: &[u8], seal: &str) -> bool {
        b64::decode(seal).is_ok_and(|tag| hmac::verify(&self.seals, text, &tag).is_ok())
    }

    pub(super) fn check_aad(&self) -> String {
        match self.id.as_str() {
            "" => check_aad(&self.vault),
            id => format!("spatikey/1/{}/chain/{id}/check", self.vault),
        }
    }

    fn before_aad(&self, prev: &str) -> String {
        format!("spatikey/1/{}/chain/{}/before/{prev}", self.vault, self.id)
    }

    /// Does `check` (base64 of nonce ‖ ciphertext ‖ tag) open as this key's?
    fn checks(&self, check: &str) -> bool {
        let Ok(mut sealed) = b64::decode(check) else { return false };
        if sealed.len() < NONCE_LEN {
            return false;
        }
        let nonce: Vec<u8> = sealed.drain(..NONCE_LEN).collect();
        self.open(&self.check_aad(), &nonce, &mut sealed).is_ok_and(|plain| plain == CHECK)
    }

    fn proof(&self) -> String {
        let mut text = PROOF.to_vec();
        text.extend_from_slice(&framed(&[&self.vault, &self.id]));
        self.sign(&text)
    }
}

impl Vault {
    fn chain_path(&self, id: &str) -> PathBuf {
        self.shared.join("chain").join(format!("{id}.json"))
    }

    /// Every change of keys in the synced folder that is one by its shape. Nothing here is
    /// believed until a key has checked it ([`Vault::ring`]).
    fn rekeys(&self) -> Vec<Rekey> {
        let dir = self.shared.join("chain");
        json_names(&dir)
            .into_iter()
            .filter(|name| is_rekey_id(name))
            .filter_map(|name| read_json::<Rekey>(&dir.join(format!("{name}.json"))).filter(|r| r.v == 1 && r.id == name && r.id != r.prev))
            .collect()
    }

    fn rekey_record(&self, id: &str) -> Option<Rekey> {
        if !is_rekey_id(id) {
            return None;
        }
        read_json::<Rekey>(&self.chain_path(id)).filter(|r| r.v == 1 && r.id == id && r.id != r.prev)
    }

    /// The changes with no change after them, among those that follow from `from` (not `from`
    /// itself): where each line of changes from there ends.
    fn leaves(&self, from: &str) -> Vec<Rekey> {
        let all = self.rekeys();
        let mut leaves = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        let mut walk: Vec<&Rekey> = all.iter().filter(|r| r.prev == from).collect();
        while let Some(record) = walk.pop() {
            if !seen.insert(record.id.clone()) {
                continue;
            }
            let after: Vec<&Rekey> = all.iter().filter(|r| r.prev == record.id).collect();
            if after.is_empty() {
                leaves.push(record.clone());
            }
            walk.extend(after);
        }
        leaves.sort_by(|a, b| a.id.cmp(&b.id));
        leaves
    }

    /// Whether the key named `id` came, by one change or several, after the one named `ancestor`.
    fn descends(&self, id: &str, ancestor: &str) -> bool {
        let mut at = id.to_string();
        for _ in 0..LONGEST_CHAIN {
            if at.is_empty() {
                return false;
            }
            let Some(record) = self.rekey_record(&at) else { return false };
            if record.prev == ancestor {
                return true;
            }
            at = record.prev;
        }
        false
    }

    /// Is `key` the vault key named `id` (empty: the first)? By what only that key opens and
    /// seals: the header's check, or the change's check and its seal over itself.
    fn is_key(&self, key: &VaultKey, id: &str) -> bool {
        let Ok(keys) = Keys::new(self.id(), id, key) else { return false };
        match id {
            "" => keys.checks(&self.header.check),
            id => self.rekey_record(id).is_some_and(|r| keys.checks(&r.check) && keys.signed(&rekey_text(self.id(), &r), &r.seal)),
        }
    }

    /// Which of this vault's keys `key` is: empty for the first, a change's name for one made
    /// since, None if it is none of them.
    pub fn generation_of(&self, key: &VaultKey) -> Option<String> {
        if self.is_key(key, "") {
            return Some(String::new());
        }
        self.rekeys().into_iter().map(|r| r.id).find(|id| self.is_key(key, id))
    }

    /// `key` and every key before it, each had from the one after and each checked as
    /// [`Vault::is_key`] checks: what a holder at `key` reads the vault with.
    pub(crate) fn ring(&self, key: &VaultKey) -> Result<Ring, String> {
        let head = self.generation_of(key).ok_or("that key does not open this vault")?;
        let mut ring = Ring { keys: vec![Keys::new(self.id(), &head, key)?], raw: vec![key.clone()] };
        let mut at = head;
        while !at.is_empty() {
            if ring.keys.len() > LONGEST_CHAIN {
                return Err("the vault's changes of keys go round in a ring".into());
            }
            let record = self.rekey_record(&at).ok_or("a change of keys is missing from the vault")?;
            let mut sealed = Zeroizing::new(b64::decode(&record.before)?);
            if sealed.len() < NONCE_LEN {
                return Err("a change of keys does not hold the key before it".into());
            }
            let nonce: Vec<u8> = sealed.drain(..NONCE_LEN).collect();
            let keys = ring.keys.last().expect("the ring has its head");
            let before = VaultKey::from_bytes(keys.open(&keys.before_aad(&record.prev), &nonce, &mut sealed[..])?)?;
            if !self.is_key(&before, &record.prev) {
                return Err("a change of keys holds a key that is not the one before it".into());
            }
            ring.keys.push(Keys::new(self.id(), &record.prev, &before)?);
            ring.raw.push(before);
            at = record.prev;
        }
        Ok(ring)
    }

    /// An entry file opened with whichever of the ring's keys it says it was sealed under. One
    /// sealed under a key that came after the ring's newest is [`REKEYED`]: the holder is behind.
    pub(crate) fn open_in_ring(&self, ring: &Ring, id: &str, bytes: &[u8]) -> Result<Entry, String> {
        let under = sealed_under(bytes)?;
        match ring.by(&under) {
            Some(keys) => open_entry(keys, id, bytes),
            None if self.descends(&under, &ring.head().id) => Err(REKEYED.into()),
            None => Err("it was sealed with a key this device does not have".into()),
        }
    }

    // ---- this holder's place among the keys ----

    fn follow(&self) -> Option<Follow> {
        read_json::<Follow>(&self.local.join("follow.json")).filter(|f| f.vault == self.header.id)
    }

    /// This device's half as it is used now: its key, and the wrap of the vault key it is at.
    pub fn half(&self) -> Option<Half> {
        let touch = self.touch_id()?;
        match self.follow().filter(|f| f.public == touch.public && !f.wrapped.is_empty()) {
            Some(follow) => Some(Half { touch: TouchId { wrapped: follow.wrapped, ..touch }, key: follow.key, proof: follow.proof }),
            None => Some(Half { touch, key: String::new(), proof: String::new() }),
        }
    }

    /// Keep which key this holder is at: the change's name (empty: the first), its wrap of it,
    /// and the proof, if the key has been had.
    pub fn set_follow(&self, public: &str, key: &str, wrapped: &str, proof: &str) -> Result<(), String> {
        let follow = Follow {
            vault: self.header.id.clone(),
            public: public.to_string(),
            key: key.to_string(),
            wrapped: wrapped.to_string(),
            proof: proof.to_string(),
        };
        write_json(&self.local.join("follow.json"), &follow)
    }

    pub(crate) fn forget_follow(&self) -> Result<(), String> {
        remove_if_there(&self.local.join("follow.json"))
    }

    /// What only `key` makes, for this holder to keep ([`Follow::proof`]). None if `key` is none
    /// of this vault's.
    pub fn proof(&self, key: &VaultKey) -> Option<String> {
        let id = self.generation_of(key)?;
        Keys::new(self.id(), &id, key).ok().map(|keys| keys.proof())
    }

    /// The changes of keys that follow from `from` and have a wrap for this holder's key, each
    /// the last of its line. One, where a holder was removed while this one was away; none,
    /// where nothing changed; more than one is for [`Vault::follows`] to tell apart.
    pub fn successors(&self, from: &str, fp: &str, public: &str) -> Vec<Successor> {
        self.leaves(from)
            .into_iter()
            .filter_map(|leaf| {
                let mine = leaf.holders.iter().find(|h| h.device.eq_ignore_ascii_case(fp) && h.public == public)?;
                Some(Successor { id: leaf.id.clone(), wrapped: mine.wrapped.clone() })
            })
            .collect()
    }

    /// Whether the keys were changed without this holder: changes follow from the key it is at,
    /// none has a wrap for it, and entries are already sealed under one of them.
    pub fn left_out(&self, from: &str, fp: &str, public: &str) -> bool {
        if self.leaves(from).is_empty() || !self.successors(from, fp, public).is_empty() {
            return false;
        }
        let dir = self.shared.join("entries");
        self.entry_ids().unwrap_or_default().iter().any(|id| {
            std::fs::read(dir.join(format!("{id}.json")))
                .ok()
                .and_then(|bytes| sealed_under(&bytes).ok())
                .is_some_and(|under| !under.is_empty() && under != from && self.descends(&under, from))
        })
    }

    /// Is `key` the key the change `leaf` made, and does it lead back -- each key sealed under the
    /// next, each checked -- to the key this holder was at (`from`, with the `proof` it kept of
    /// it, if it kept one)?
    pub fn follows(&self, key: &VaultKey, leaf: &str, from: &str, proof: &str) -> bool {
        let Ok(ring) = self.ring(key) else { return false };
        if ring.head().id != leaf {
            return false;
        }
        match ring.by(from) {
            Some(keys) if proof.is_empty() => keys.id == from,
            Some(keys) => {
                let mut text = PROOF.to_vec();
                text.extend_from_slice(&framed(&[&self.header.id, from]));
                keys.signed(&text, proof)
            }
            None => false,
        }
    }

    // ---- holders' records ----

    /// Seal a holder's record under `key`, the key it holds a wrap of: only a device that has had
    /// that key says who else holds it.
    pub fn seal_holder(&self, key: &VaultKey, device: &mut TouchDevice) -> Result<(), String> {
        let id = self.generation_of(key).ok_or("that key does not open this vault")?;
        let keys = Keys::new(self.id(), &id, key)?;
        device.seal = keys.sign(&holder_text(self.id(), &id, device));
        device.key = id;
        Ok(())
    }

    /// Whether a holder's record was sealed under `key` as it stands: its device, its kind and
    /// its public key.
    pub fn holder_sealed(&self, key: &VaultKey, device: &TouchDevice) -> bool {
        let Some(id) = self.generation_of(key) else { return false };
        let Ok(keys) = Keys::new(self.id(), &id, key) else { return false };
        device.key == id && !device.seal.is_empty() && keys.signed(&holder_text(self.id(), &id, device), &device.seal)
    }

    // ---- the recovery code among the keys ----

    fn recovery_file(&self) -> Option<RecoveryRecord> {
        read_json::<RecoveryRecord>(&self.shared.join("recovery.json"))
    }

    /// The record that wraps the key named `id` to the recovery code: `recovery.json` when it is
    /// for that key (a code made since the change), else the change's own.
    fn recovery_for(&self, id: &str) -> Option<RecoveryRecord> {
        match self.recovery_file() {
            Some(file) if file.key == id => Some(file),
            _ => self.rekey_record(id).and_then(|r| r.recovery),
        }
    }

    pub(crate) fn any_recovery(&self) -> bool {
        self.shared.join("recovery.json").is_file() || self.rekeys().iter().any(|r| r.recovery.is_some())
    }

    /// The newest vault key, from the recovery code: where the keys were changed, the last of the
    /// line, and [`RIVALS`] where there are two lines the code opens.
    pub(crate) fn recover_newest(&self, code: &str) -> Result<VaultKey, String> {
        let wrong = || recovery::WRONG_CODE.to_string();
        let leaves = self.leaves("");
        if leaves.is_empty() {
            let record = self.recovery_file().ok_or("this vault has no recovery code")?;
            let key = recovery::unwrap(&record, code, self.id())?;
            if !self.is_key(&key, "") {
                return Err("the recovery code gave a key that does not open this vault".into());
            }
            return Ok(key);
        }
        let mut found: Vec<VaultKey> = Vec::new();
        let mut any = false;
        for leaf in &leaves {
            let Some(record) = self.recovery_for(&leaf.id) else { continue };
            any = true;
            let Ok(key) = recovery::unwrap(&record, code, self.id()) else { continue };
            if self.ring(&key).is_ok_and(|ring| ring.head().id == leaf.id) {
                found.push(key);
            }
        }
        match found.len() {
            0 if !any => Err("this vault has no recovery code for the keys it has now: make one on a device that opens SpatiKey".into()),
            0 => Err(wrong()),
            1 => Ok(found.remove(0)),
            _ => Err(RIVALS.into()),
        }
    }

    // ---- changing the keys ----

    /// Make a new vault key, the holders in `removed` left out of it: [`Rekey`] written, and
    /// nothing else. `key` must be the newest; `me` is this holder, whose half says its public
    /// key. What follows -- this holder's own place, the entries, the tombstones, the records,
    /// `recovery.json` -- is [`Vault::settle`]'s, here and on every other holder.
    pub fn rekey(
        &self,
        key: &VaultKey,
        removed: &[String],
        me: &str,
        by: &str,
        plan: RecoveryPlan<'_>,
        cost: (u32, u32, u32),
    ) -> Result<Rekeyed, String> {
        let ring = self.ring(key)?;
        let prev = ring.head().id.clone();
        if !self.leaves(&prev).is_empty() {
            return Err(REKEYED.into());
        }
        let gone = |fp: &str| removed.iter().any(|r| r.eq_ignore_ascii_case(fp));
        if gone(me) {
            return Err("a device does not remove itself".into());
        }
        let mine = self.half().ok_or("this device does not open the vault")?;
        let new = VaultKey::random();
        let id = random_hex(8);
        let keys = Keys::new(self.id(), &id, &new)?;

        // Who stays: this holder by its own half, the others by records a holder sealed.
        let stayed = self.rekey_record(&prev).map(|r| r.holders).unwrap_or_default();
        let mut left_out = Vec::new();
        let mut holders: Vec<(String, String)> = vec![(me.to_ascii_lowercase(), mine.touch.public.clone())];
        for device in self.touch_devices() {
            if device.device.eq_ignore_ascii_case(me) || gone(&device.device) {
                continue;
            }
            // Sealed under this key; or wrapped to by the change that made this key, whose own
            // seal says so (its records not yet sealed again: a change broken off).
            let kept = |d: &TouchDevice| stayed.iter().any(|h| h.device.eq_ignore_ascii_case(&d.device) && h.public == d.public);
            if holders::known(&device.kind) && (self.holder_sealed(key, &device) || kept(&device)) {
                holders.push((device.device.to_ascii_lowercase(), device.public.clone()));
            } else {
                left_out.push(device.name.clone());
            }
        }
        holders.sort();
        let mut wraps = Vec::with_capacity(holders.len());
        for (device, public) in holders {
            let wrapped = holders::wrap(&new, &b64::decode(&public)?)?;
            wraps.push(RekeyHolder { device, public, wrapped: b64::encode(&wrapped) });
        }

        // The recovery code: wrapped to again where it can be (version 2), kept by being typed
        // or replaced where it cannot (version 1).
        let mut recovery_code = None;
        let recovery = match self.recovery_for(&prev) {
            None => None,
            Some(record) if record.rewraps() => {
                // No code is needed; one typed all the same is to be the vault's.
                if let RecoveryPlan::Keep(code) = plan {
                    if !recovery::unwrap(&record, code, self.id()).is_ok_and(|opened| opened.bytes() == key.bytes()) {
                        return Err(recovery::WRONG_CODE.into());
                    }
                }
                Some(recovery::rewrap(&record, &new, self.id(), &id)?)
            }
            Some(record) => {
                let code = match plan {
                    RecoveryPlan::Keep(code) => {
                        if !recovery::unwrap(&record, code, self.id()).is_ok_and(|opened| opened.bytes() == key.bytes()) {
                            return Err(recovery::WRONG_CODE.into());
                        }
                        Zeroizing::new(code.to_string())
                    }
                    RecoveryPlan::New => {
                        let code = Zeroizing::new(recovery::new_code());
                        recovery_code = Some(code.clone());
                        code
                    }
                };
                Some(recovery::wrap_to_code(&new, &code, self.id(), &id, cost)?)
            }
        };

        let mut before = Zeroizing::new(Vec::with_capacity(32 + CHACHA20_POLY1305.tag_len()));
        before.extend_from_slice(key.bytes());
        let nonce = keys.seal(&keys.before_aad(&prev), &mut before)?;
        let mut sealed_before = nonce.to_vec();
        sealed_before.extend_from_slice(&before);

        let mut removed: Vec<String> = removed.iter().map(|fp| fp.to_ascii_lowercase()).collect();
        removed.sort();
        removed.dedup();
        let mut record = Rekey {
            v: 1,
            id: id.clone(),
            prev,
            made: crate::now_ms(),
            by: by.to_string(),
            removed,
            check: keys.seal_check()?,
            before: b64::encode(&sealed_before),
            holders: wraps,
            recovery,
            seal: String::new(),
        };
        record.seal = keys.sign(&rekey_text(self.id(), &record));
        // The one write that is the change.
        write_json(&self.chain_path(&id), &record)?;
        log::info!("spatikey: vault {} has new keys ({id}), for {} holders", self.header.id, record.holders.len());
        Ok(Rekeyed { key: new, id, recovery_code, left_out })
    }

    /// Finish whatever a change of keys left to do, with the newest key in hand; every step is
    /// one that doing twice changes nothing. `me` is this holder (empty for none: the recovery
    /// code on its way to setting one up). Says how many entries were sealed again.
    pub fn settle(&self, key: &VaultKey, me: &str) -> Result<usize, String> {
        let ring = self.ring(key)?;
        let head = ring.head().id.clone();
        // This holder's own place: its wrap of the newest key, and the proof of it.
        if let Some(half) = self.half() {
            if half.key == head {
                if half.proof.is_empty() {
                    self.set_follow(&half.touch.public, &head, &half.touch.wrapped, &ring.head().proof())?;
                }
            } else if let Some(mine) = self
                .rekey_record(&head)
                .and_then(|r| r.holders.into_iter().find(|h| h.device.eq_ignore_ascii_case(me) && h.public == half.touch.public))
            {
                self.set_follow(&mine.public, &head, &mine.wrapped, &ring.head().proof())?;
            }
        }
        if let Some(record) = self.rekey_record(&head) {
            // Whoever was removed along the way is a tombstone -- unless a holder has added it
            // again since, which its record sealed under the newest key says.
            for id in ring.keys.iter().map(|k| k.id.as_str()).filter(|id| !id.is_empty()) {
                for fp in self.rekey_record(id).map(|r| r.removed).unwrap_or_default() {
                    if self.touch_device(&fp).is_some_and(|d| !d.removed && !self.holder_sealed(key, &d)) {
                        self.remove_touch_device(&fp)?;
                    }
                }
            }
            // The holders that stayed: their records say the newest key, sealed under it.
            for holder in &record.holders {
                let Some(mut device) = self.touch_device(&holder.device) else { continue };
                if device.removed || device.public != holder.public || (device.wrapped == holder.wrapped && self.holder_sealed(key, &device)) {
                    continue;
                }
                device.wrapped = holder.wrapped.clone();
                self.seal_holder(key, &mut device)?;
                self.add_touch_device(&device)?;
            }
            // The recovery code's record, where the change carries the newer one.
            if let Some(recovery) = &record.recovery {
                if self.recovery_file().is_none_or(|file| file.key != head) {
                    write_json(&self.shared.join("recovery.json"), recovery)?;
                }
            }
        }
        Ok(self.sweep(&ring))
    }

    /// Seal again, under the ring's newest key, every entry sealed under an older one. The
    /// entry is what it was -- its version, when it changed and by whom -- and its file's time
    /// moves a millisecond: newer than the copies of it elsewhere, older than a change made since.
    fn sweep(&self, ring: &Ring) -> usize {
        let mut sealed_again = 0;
        for id in self.entry_ids().unwrap_or_default() {
            let path = self.entry_path(&id);
            let Ok(bytes) = std::fs::read(&path) else { continue };
            let Ok(under) = sealed_under(&bytes) else { continue };
            if under == ring.head().id {
                continue;
            }
            let Some(keys) = ring.by(&under) else { continue };
            let mut entry = match open_entry(keys, &id, &bytes) {
                Ok(entry) => entry,
                Err(e) => {
                    log::warn!("spatikey: entry {id} does not open with the key it names ({e}); left as it is");
                    continue;
                }
            };
            let sealed = seal_entry(ring.head(), &entry);
            entry.wipe();
            let was = std::fs::metadata(&path).and_then(|m| m.modified()).map(millis).unwrap_or(0);
            match sealed.and_then(|bytes| write_synced_at(&path, &bytes, Some(was + 1)).map_err(|e| e.to_string())) {
                Ok(()) => sealed_again += 1,
                Err(e) => log::warn!("spatikey: entry {id} could not be sealed under the new key: {e}"),
            }
        }
        if sealed_again > 0 {
            log::info!("spatikey: {sealed_again} entries of vault {} sealed under its newest key", self.header.id);
        }
        sealed_again
    }
}

/// The name of the key an entry file says it was sealed under: empty for the vault's first.
fn sealed_under(bytes: &[u8]) -> Result<String, String> {
    let file: Sealed = serde_json::from_slice(bytes).map_err(|e| format!("not an entry file: {e}"))?;
    Ok(file.k)
}

#[cfg(test)]
#[path = "rekey_tests.rs"]
mod tests;
