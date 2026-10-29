//! `spatikey --demo`: SpatiKey's screens with nothing behind them but this.
//!
//! A stand-in for the service inside the window's own process, answering the same requests with
//! made-up entries, so that every screen can be seen and photographed in the test room without a
//! vault, a Touch ID or a Mac. A touch is a pause; nothing here is real, and nothing is kept.
//!
//! ```text
//! --demo            a Mac whose vault is set up, locked
//! --demo=setup      a Mac with no vault yet
//! --demo=deck       a Steam Deck paired with no Mac
//! --demo=new-mac    a Mac the vault has reached, whose Touch ID has no key to it yet
//! --demo=fingers    a Mac whose enrolled fingers changed since its key was made
//! --demo=away       no service at all: Spatiand is not running
//! ```

use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};
use spatikey_core::entry::{Entry, Kind, Passkey, Totp};
use spatikey_core::wire::{
    Approver, Code, Device, DevicesAnswer, FindAnswer, Found, GetAnswer, IdAnswer, ListAnswer, OpenAnswer, PairAnswer, Refusal, Request,
    SettingsAnswer, SetupAnswer, StatusAnswer, TargetAnswer, TotpAnswer,
};

use crate::ask::{Service, Trouble};

const MAC: (&str, &str) = ("darkPhoenix", "0b8dfc23a4e15b7790c2d6e81f3a5c4b9d0e7f6a5b4c3d2e1f0a9b8c7d6e5f40");
const DECK: (&str, &str) = ("Steam Deck", "5c1e9a7b3d2f4e6a8b0c1d2e3f4a5b6c7d8e9f0a1b2c3d4e5f6a7b8c9d0e1f2a");
const BEAM: (&str, &str) = ("Beam Pro", "9f8e7d6c5b4a39281706f5e4d3c2b1a0918273645546372819a0b1c2d3e4f5a6");
/// Shown at setting up, and taken back by "Set up Touch ID again": made up, of Crockford's
/// alphabet, as a real one is.
const FIRST_CODE: &str = "7KQ2M-X9D4F-H3TNB-W6PZR-0CJ8V";
const NEW_CODE: &str = "M4R8T-2BWQ9-KD7XC-5HVNE-1PJ6S";

pub struct Fake {
    mac: bool,
    me: (&'static str, &'static str),
    vault: bool,
    touch_here: bool,
    approvers: Vec<Approver>,
    entries: Vec<Entry>,
    session: Option<String>,
    sessions: u32,
    settings: spatikey_core::wire::Settings,
    allowed: Vec<Device>,
    code: &'static str,
    made: u32,
    /// Touch ID's fingers changed: opening is refused until the recovery code is given.
    fingers_changed: bool,
    /// Answers nothing, as when the service is not running.
    away: bool,
}

impl Fake {
    pub fn new(scenario: &str) -> Fake {
        let mac = scenario != "deck";
        let mut fake = Fake {
            mac,
            me: if mac { MAC } else { DECK },
            vault: scenario != "setup",
            touch_here: !matches!(scenario, "setup" | "new-mac" | "deck"),
            approvers: Vec::new(),
            entries: entries(),
            session: None,
            sessions: 0,
            settings: Default::default(),
            allowed: vec![device(DECK, 3), device(BEAM, 1)],
            code: FIRST_CODE,
            made: 0,
            fingers_changed: scenario == "fingers",
            away: scenario == "away",
        };
        if scenario == "setup" {
            fake.entries.clear();
        }
        if !matches!(scenario, "" | "mac" | "setup" | "deck" | "new-mac" | "fingers" | "away") {
            log::warn!("--demo={scenario}: not a demo there is; showing the Mac's");
        }
        fake
    }

    fn status(&self) -> StatusAnswer {
        StatusAnswer {
            vault: self.vault,
            opens_here: self.mac && self.touch_here,
            can_set_up: self.mac && !(self.vault && self.touch_here),
            approvers: self.approvers.clone(),
            name: self.me.0.into(),
            fp: self.me.1.into(),
        }
    }

    fn session(&self, session: &str) -> Result<(), Trouble> {
        match &self.session {
            Some(open) if open == session => Ok(()),
            _ => Err(refused(Code::BadSession, "")),
        }
    }

    fn live(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(|e| !e.deleted)
    }

    fn entry(&self, id: &str) -> Result<&Entry, Trouble> {
        self.live().find(|e| e.id == id).ok_or_else(|| refused(Code::NoMatch, "no such entry"))
    }

    /// The Mac a Steam Deck asks, once it has paired, as a Mac answers: after the finger.
    fn touch(&self) -> Result<(), Trouble> {
        if !self.mac && self.approvers.is_empty() {
            return Err(refused(Code::NoApprover, ""));
        }
        std::thread::sleep(Duration::from_millis(if self.mac { 900 } else { 1500 }));
        Ok(())
    }
}

impl Service for Fake {
    fn ask(&mut self, request: Request) -> Result<Value, Trouble> {
        if self.away {
            return Err(Trouble::Away("No such file or directory (os error 2)".into()));
        }
        match request {
            Request::Status => ok(self.status()),
            Request::Setup { .. } => {
                if self.vault {
                    return Err(refused(Code::VaultExists, ""));
                }
                std::thread::sleep(Duration::from_millis(1200));
                self.vault = true;
                self.touch_here = true;
                ok(SetupAnswer { recovery_code: self.code.into() })
            }
            Request::Open => {
                if !self.vault && self.mac {
                    return Err(refused(Code::NoVault, ""));
                }
                if self.mac && (!self.touch_here || self.fingers_changed) {
                    return Err(refused(Code::FingerprintsChanged, ""));
                }
                self.touch()?;
                self.sessions += 1;
                let session = format!("demo-{}", self.sessions);
                self.session = Some(session.clone());
                ok(OpenAnswer { session, entries: self.live().map(Entry::summary).collect() })
            }
            Request::Close { .. } => {
                self.session = None;
                ok(json!({}))
            }
            Request::List { session } => {
                self.session(&session)?;
                ok(ListAnswer { entries: self.live().map(Entry::summary).collect() })
            }
            Request::Get { session, id } => {
                self.session(&session)?;
                ok(GetAnswer { entry: self.entry(&id)?.clone() })
            }
            Request::Put { session, mut entry } => {
                self.session(&session)?;
                let now = spatikey_core::now_ms();
                if entry.id.is_empty() {
                    self.made += 1;
                    entry.id = format!("{:032x}", 0xde70_0000u64 + self.made as u64);
                    entry.created = now;
                }
                entry.changed = now;
                entry.changed_by = self.me.1[..8].into();
                entry.version += 1;
                let id = entry.id.clone();
                match self.entries.iter_mut().find(|e| e.id == id) {
                    Some(kept) => *kept = entry,
                    None => self.entries.push(entry),
                }
                ok(IdAnswer { id })
            }
            Request::Delete { session, id } => {
                self.session(&session)?;
                self.entries.retain(|e| e.id != id);
                ok(json!({}))
            }
            Request::Totp { session, id } => {
                match session {
                    Some(session) => self.session(&session)?,
                    None => self.touch()?,
                }
                let entry = self.entry(&id)?;
                let period = entry.totp.as_ref().map_or(30, |t| t.period.max(1));
                let now = spatikey_core::now_ms() / 1000;
                ok(TotpAnswer { code: made_up_code(&id, now / period), remaining: period - now % period, period })
            }
            Request::Type { session, .. } => {
                match session {
                    Some(session) => self.session(&session)?,
                    None => self.touch()?,
                }
                std::thread::sleep(Duration::from_millis(500));
                ok(json!({}))
            }
            Request::Target => ok(TargetAnswer { window: 41, app: "steam".into(), title: "Steam".into() }),
            Request::RecoveryNew { session } => {
                self.session(&session)?;
                self.code = NEW_CODE;
                ok(SetupAnswer { recovery_code: self.code.into() })
            }
            Request::Recover { code } => {
                std::thread::sleep(Duration::from_millis(900));
                let plain = |c: &str| c.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_uppercase()).collect::<String>();
                if plain(&code) != plain(self.code) {
                    return Err(refused(Code::BadRequest, ""));
                }
                self.touch_here = true;
                self.fingers_changed = false;
                ok(json!({}))
            }
            Request::Devices { session } => {
                self.session(&session)?;
                ok(DevicesAnswer { allowed: self.allowed.clone(), touch_ids: vec![device(MAC, 9)] })
            }
            Request::ForgetDevice { session, fp } => {
                self.session(&session)?;
                self.allowed.retain(|d| d.fp != fp);
                ok(json!({}))
            }
            Request::Settings { session, set } => {
                self.session(&session)?;
                if let Some(set) = set {
                    self.settings = set;
                }
                ok(SettingsAnswer { settings: self.settings })
            }
            Request::Find => {
                if self.mac {
                    return Err(refused(Code::NotHere, "a Mac opens SpatiKey itself"));
                }
                std::thread::sleep(Duration::from_millis(1100));
                let paired = self.approvers.iter().any(|a| a.fp == MAC.1);
                ok(FindAnswer { macs: vec![Found { fp: MAC.1.into(), name: MAC.0.into(), address: "192.168.1.20".into(), paired }] })
            }
            Request::PairWith { fp } => {
                if fp != MAC.1 {
                    return Err(refused(Code::NotReachable, ""));
                }
                // The Mac's owner reading the digits and saying yes.
                std::thread::sleep(Duration::from_millis(2500));
                ok(PairAnswer { code: crate::ui::pairing_code(self.me.1, MAC.1).unwrap_or_default(), paired: true })
            }
            Request::PairConfirm { fp, ok: yes } => {
                if yes && fp == MAC.1 && !self.approvers.iter().any(|a| a.fp == fp) {
                    self.approvers.push(Approver { fp, name: MAC.0.into(), near: true });
                }
                ok(json!({}))
            }
            _ => Err(refused(Code::NotHere, "not in the demo")),
        }
    }
}

fn ok(answer: impl Serialize) -> Result<Value, Trouble> {
    serde_json::to_value(answer).map_err(|e| refused(Code::Failed, &e.to_string()))
}

fn refused(code: Code, message: &str) -> Trouble {
    Trouble::Refused(Refusal::new(code, message))
}

fn device((name, fp): (&str, &str), days_ago: u64) -> Device {
    Device { fp: fp.into(), name: name.into(), added: spatikey_core::now_ms().saturating_sub(days_ago * 86_400_000) }
}

/// Six digits that change with every period, different for each entry: not RFC 6238, which
/// is the service's to do with a real secret.
fn made_up_code(id: &str, step: u64) -> String {
    let seed = id.bytes().fold(step.wrapping_mul(0x9e37_79b9_7f4a_7c15), |h, b| (h ^ b as u64).wrapping_mul(0x0100_0000_01b3));
    format!("{:06}", (seed >> 17) % 1_000_000)
}

/// The made-up vault: logins for sites and for a window, two passkeys, a note.
fn entries() -> Vec<Entry> {
    let now = spatikey_core::now_ms();
    let day = 86_400_000;
    let code = |issuer: &str, label: &str| Some(Totp { secret: "JBSWY3DPEHPK3PXP".into(), digits: 6, period: 30, algorithm: "SHA1".into(), issuer: issuer.into(), label: label.into() });
    let login = |n: u64, title: &str, sites: &[&str], username: &str, password: &str| Entry {
        id: format!("{:032x}", 0x5ca1_0000u64 + n),
        kind: Kind::Login,
        title: title.into(),
        sites: sites.iter().map(|s| s.to_string()).collect(),
        username: username.into(),
        password: password.into(),
        created: now - (40 + n) * day,
        changed: now - n * day,
        changed_by: MAC.1[..8].into(),
        version: 1,
        ..Entry::default()
    };
    let passkey = |n: u64, title: &str, rp: &str, user: &str| Entry {
        id: format!("{:032x}", 0x5ca1_0000u64 + n),
        kind: Kind::Passkey,
        title: title.into(),
        sites: vec![rp.into()],
        passkey: Some(Passkey {
            rp_id: rp.into(),
            rp_name: title.into(),
            credential_id: "q1w2e3r4t5y6u7i8o9p0aa".into(),
            user_handle: "dXNlci0xMjM0".into(),
            user_name: user.into(),
            user_display: "Paulus".into(),
            key: String::new(),
            alg: -7,
            sign_count: 0,
        }),
        created: now - (30 + n) * day,
        changed: now - (30 + n) * day,
        changed_by: MAC.1[..8].into(),
        version: 1,
        ..Entry::default()
    };
    let mut github = login(1, "GitHub", &["github.com"], "paulus", "demo-only-Gh#71xQ");
    github.totp = code("GitHub", "paulus");
    github.notes = "Recovery codes are printed and in the drawer.".into();
    let mut steam = login(2, "Steam", &["steampowered.com", "steamcommunity.com"], "paulus_p", "demo-only-St@m42");
    steam.apps = vec!["steam".into()];
    let mut proton = login(3, "Proton Mail", &["proton.me"], "paulus@proton.me", "demo-only-Pr0ton!");
    proton.totp = code("Proton", "paulus@proton.me");
    let router = login(4, "Home router", &["https://192.168.1.1"], "admin", "demo-only-R0uter");
    let mut hetzner = login(5, "Hetzner", &["hetzner.com"], "paulus", "demo-only-Hz_88k");
    hetzner.totp = code("Hetzner", "paulus");
    let jellyfin = login(6, "Jellyfin", &["https://films.example.org"], "paulus", "demo-only-Jf9!");
    let note = Entry {
        id: format!("{:032x}", 0x5ca1_0009u64),
        kind: Kind::Note,
        title: "Wi-Fi at home".into(),
        notes: "Network: Rumah-5G\nThe password is on the sticker under the router.".into(),
        created: now - 90 * day,
        changed: now - 12 * day,
        changed_by: MAC.1[..8].into(),
        version: 1,
        ..Entry::default()
    };
    vec![
        github,
        steam,
        proton,
        router,
        hetzner,
        jellyfin,
        passkey(7, "Google", "google.com", "paulus@example.com"),
        passkey(8, "Tailscale", "login.tailscale.com", "paulus@example.com"),
        note,
    ]
}
