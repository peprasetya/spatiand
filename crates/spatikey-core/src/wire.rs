//! The protocol every caller speaks to the service, on the local socket and between devices.
//!
//! Each message is a 4-byte big-endian length, then JSON. A request is
//! `{"id": n, "op": ..., ...}`; its answer is `{"id": n, "ok": {...}}` or
//! `{"id": n, "err": {"code": ..., "message": ...}}`. JSON, because the browser extensions and the
//! Swift half speak it too. The answers' shapes are the `*Answer` structs below, sent as `ok`.

use std::io::{Read, Write};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::entry::{Entry, Summary};

/// Larger than this is not a request; the connection is closed.
pub const MAX_MESSAGE: usize = 512 * 1024;

/// Written with the request's fields beside `id` and `who`; read by hand (its `Deserialize`
/// below), so that the requests naming an entry are read as builds before 2026-11-18 wrote them
/// too, with the entry's id as a second `id` (which no build could read).
#[derive(Debug, Clone, Serialize)]
pub struct Envelope {
    pub id: u64,
    /// Who is asking, as the end that received it names them. A caller may fill in `app`; the
    /// receiving end adds what it knows itself (the peer process, the paired device's name).
    #[serde(default)]
    pub who: Who,
    #[serde(flatten)]
    pub request: Request,
}

impl Request {
    /// Overwrite what a request carries that is secret -- an entry, a password, a recovery
    /// code -- once it has been written out: what asks is done with it.
    pub fn wipe(&mut self) {
        use zeroize::Zeroize;
        match self {
            Request::Put { entry, .. } => entry.wipe(),
            Request::Save { password, .. } => password.zeroize(),
            Request::Recover { code } | Request::RecoverySaved { code } => code.zeroize(),
            Request::ForgetDevice { recovery_code: Some(code), .. } => code.zeroize(),
            _ => {}
        }
    }
}

impl<'de> Deserialize<'de> for Envelope {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Envelope, D::Error> {
        struct Fields;
        impl<'de> serde::de::Visitor<'de> for Fields {
            type Value = Envelope;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a request")
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<Envelope, A::Error> {
                use serde::de::Error;
                let (mut id, mut who, mut rest) = (None, None, serde_json::Map::new());
                // The entry's id, as an older build sent it: a second `id`, a string.
                let mut older = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "id" => match map.next_value::<Value>()? {
                            Value::Number(n) if id.is_none() => {
                                id = Some(n.as_u64().ok_or_else(|| A::Error::custom("the id is not a whole number"))?)
                            }
                            Value::String(entry) if older.is_none() => older = Some(entry),
                            _ => return Err(A::Error::duplicate_field("id")),
                        },
                        "who" if who.is_none() => who = Some(map.next_value::<Who>()?),
                        _ if key == "who" || rest.contains_key(&key) => {
                            return Err(A::Error::custom(format!("duplicate field `{key}`")))
                        }
                        _ => {
                            let value = map.next_value::<Value>()?;
                            rest.insert(key, value);
                        }
                    }
                }
                let id = id.ok_or_else(|| A::Error::missing_field("id"))?;
                if let Some(entry) = older {
                    if rest.contains_key("entry") {
                        return Err(A::Error::duplicate_field("entry"));
                    }
                    rest.insert("entry".into(), Value::String(entry));
                }
                let request = Request::deserialize(Value::Object(rest)).map_err(A::Error::custom)?;
                Ok(Envelope { id, who: who.unwrap_or_default(), request })
            }
        }
        deserializer.deserialize_map(Fields)
    }
}

/// A message's own id, for the refusal of what could not be read as a request: its first `id`
/// that is a whole number; 0 when there is none, or it is not JSON.
pub fn id_of(frame: &[u8]) -> u64 {
    struct First;
    impl<'de> serde::de::Visitor<'de> for First {
        type Value = u64;

        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("an object")
        }

        fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<u64, A::Error> {
            let mut found = None;
            while let Some(key) = map.next_key::<std::borrow::Cow<str>>()? {
                if key == "id" && found.is_none() {
                    found = map.next_value::<Value>()?.as_u64();
                } else {
                    map.next_value::<serde::de::IgnoredAny>()?;
                }
            }
            Ok(found.unwrap_or(0))
        }
    }
    let mut reader = serde_json::Deserializer::from_slice(frame);
    serde::Deserializer::deserialize_map(&mut reader, First).unwrap_or(0)
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Who {
    /// The program: "SpatiMesh", "SpatiKey", "Chrome".
    #[serde(default)]
    pub app: String,
    /// The device, when it is not this one: "Steam Deck". Empty for this machine.
    #[serde(default)]
    pub device: String,
}

/// The fields of an entry that can be typed or revealed, and the keys between them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Field {
    Username,
    Password,
    /// The current one-time code.
    Totp,
    Tab,
    Enter,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    /// Is there a vault, can this machine open it, which Macs could approve. No touch.
    Status,
    /// Make the vault on this Mac and set Touch ID up. Answers [`SetupAnswer`].
    Setup {
        #[serde(default)]
        name: String,
    },
    /// Open SpatiKey: one touch, then a session. Answers [`OpenAnswer`].
    Open,
    Close {
        session: String,
    },
    /// Answers [`ListAnswer`].
    List {
        session: String,
    },
    /// Answers [`GetAnswer`].
    Get {
        session: String,
        /// The entry's: `entry` on the wire, as `id` is the envelope's (and so in the others).
        #[serde(rename = "entry")]
        id: String,
    },
    /// Add (empty id) or change an entry. Answers [`IdAnswer`].
    Put {
        session: String,
        entry: Entry,
        /// The `changed` of the entry as the change was begun from: given, the entry is written
        /// only if it is still that one, else refused `stale` (another device changed or deleted
        /// it meanwhile). Not given (a window before 2026-11-18, a new entry), written whatever is
        /// there, as before.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        based_on: Option<u64>,
    },
    Delete {
        session: String,
        #[serde(rename = "entry")]
        id: String,
    },
    /// The current code. With no session, asks for a touch. Answers [`TotpAnswer`].
    Totp {
        #[serde(default)]
        session: Option<String>,
        #[serde(rename = "entry")]
        id: String,
    },
    /// Fields of one entry, for typing them. With no session, asks for a touch.
    /// Answers [`RevealAnswer`], in the order asked (`tab` and `enter` give empty strings).
    Reveal {
        #[serde(default)]
        session: Option<String>,
        #[serde(rename = "entry")]
        id: String,
        fields: Vec<Field>,
    },
    /// The logins for a page. Asks for a touch. Answers [`FillAnswer`].
    Fill {
        origin: String,
    },
    /// Keep a login a page has just used. Asks for a touch. Answers [`IdAnswer`].
    Save {
        origin: String,
        #[serde(default)]
        title: String,
        username: String,
        password: String,
    },
    /// A finger's yes to opening a browser profile's locked windows, every time: nothing of the
    /// vault leaves, and a touch that lingers for other things does not count. Answers `{}`.
    ProfileUnlock {
        profile: String,
    },
    /// Answers [`CreatedAnswer`], or the error `excluded`.
    PasskeyCreate(PasskeyCreate),
    /// Answers [`ChoicesAnswer`] (one signed answer per account the site has here, newest
    /// first, so that one touch covers choosing between them), or the error `no_match`.
    PasskeyGet(PasskeyGet),
    /// Type an entry's fields into a window (by default [`Request::Target`]'s). Answers `{}`.
    Type {
        #[serde(default)]
        session: Option<String>,
        #[serde(rename = "entry")]
        id: String,
        fields: Vec<Field>,
        #[serde(default)]
        window: Option<u64>,
    },
    /// The window the wearer was in before SpatiKey's own. Answers [`TargetAnswer`].
    Target,
    /// Answers [`SetupAnswer`] (the new code).
    RecoveryNew {
        session: String,
    },
    /// Set Touch ID up on this Mac again from the recovery code. Answers `{}`.
    Recover {
        code: String,
    },
    /// The wearer said the recovery code shown is saved: it is not shown again at the next
    /// opening ([`OpenAnswer::recovery_code`]). The code itself, as only one who saw it has it.
    /// Answers `{}`; a service before 2026-11-18 refuses it `bad_request`, which is no matter.
    RecoverySaved {
        code: String,
    },
    /// Answers [`DevicesAnswer`].
    Devices {
        session: String,
    },
    /// Answers [`ForgetAnswer`]. Removing a holder makes the vault new keys (docs/spatikey.md,
    /// "Removing a holder makes new keys"). `recovery_code` is the vault's recovery code, for a
    /// vault whose record of it is from before 2026-11-18: given, the code goes on opening the
    /// vault; not given, a new one is made and answered. A newer vault needs neither.
    ForgetDevice {
        session: String,
        fp: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        recovery_code: Option<String>,
    },
    /// Answers [`SettingsAnswer`]; with `set`, changes them first.
    Settings {
        session: String,
        #[serde(default)]
        set: Option<Settings>,
    },
    /// Between devices only: a device asking to be let in. Answers `{"paired": true}`.
    Pair {
        name: String,
    },
    /// On a device: look for Macs on this network. Answers [`FindAnswer`].
    Find,
    /// On a device: pair with the Mac `fp` that [`Request::Find`] found. Answers [`PairAnswer`]
    /// once the Mac's owner has answered; nothing is written down on this device until
    /// [`Request::PairConfirm`].
    PairWith {
        fp: String,
    },
    /// On a device: the wearer saw the same six digits on the Mac (or did not). Answers `{}`.
    PairConfirm {
        fp: String,
        ok: bool,
    },
    /// On a device with biometrics that does not open the vault yet: make its key and ask, in the
    /// synced folder, to be added by a device that does. Asked again while it waits, the same
    /// request and the same digits. Answers [`HolderAnswer`].
    HolderRequest {
        #[serde(default)]
        name: String,
    },
    /// On a holder, in a session: add the device `fp` that asked, if it shows `code`. Asks for a
    /// new touch, whatever the session or a lingering touch would cover. Answers `{}`.
    HolderApprove {
        session: String,
        fp: String,
        code: String,
    },
    /// On a holder, in a session: turn the request of the device `fp` down. Answers `{}`.
    HolderRefuse {
        session: String,
        fp: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PasskeyCreate {
    /// As the engine or the browser reported it, never as the page claimed.
    pub origin: String,
    pub rp_id: String,
    #[serde(default)]
    pub rp_name: String,
    /// base64url.
    pub user_id: String,
    pub user_name: String,
    #[serde(default)]
    pub user_display: String,
    /// base64url of SHA-256(clientDataJSON).
    pub client_data_hash: String,
    /// base64url credential ids the site already has for this account.
    #[serde(default)]
    pub exclude: Vec<String>,
    /// The COSE algorithms the site accepts, in its order of preference.
    #[serde(default)]
    pub algs: Vec<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PasskeyGet {
    pub origin: String,
    pub rp_id: String,
    /// base64url of SHA-256(clientDataJSON).
    pub client_data_hash: String,
    /// base64url credential ids the site will take; empty for any of this site's.
    #[serde(default)]
    pub allow: Vec<String>,
    /// Offered in a login field rather than asked for by a button.
    #[serde(default)]
    pub conditional: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct Settings {
    /// How long one touch covers what follows: 0 (every use), 60 or 300 seconds.
    pub touch_secs: u64,
    /// How long an open SpatiKey stays open unused.
    pub open_secs: u64,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings { touch_secs: 0, open_secs: 300 }
    }
}

// ---- answers ----

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct StatusAnswer {
    pub vault: bool,
    /// This machine opens it itself (a Mac with Touch ID set up for it).
    pub opens_here: bool,
    /// This machine could set it up (a Mac with Touch ID and no vault yet, or a new Mac).
    pub can_set_up: bool,
    pub approvers: Vec<Approver>,
    /// This machine's name.
    pub name: String,
    /// This machine's fingerprint, in full (the pairing code is made from both ends').
    pub fp: String,
    /// The devices that open the vault with their own biometrics, oldest first.
    #[serde(default)]
    pub holders: Vec<Holder>,
    /// What this machine's own biometrics are, when it has any: `mac-touch-id`,
    /// `android-biometric`; empty for none (the Deck, the Beam Pro).
    #[serde(default)]
    pub kind: String,
    /// While this machine waits to be added as a holder: the six digits it shows.
    #[serde(default)]
    pub request_code: String,
}

/// A device that opens the vault with its own biometrics.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Holder {
    pub fp: String,
    pub name: String,
    /// `mac-touch-id` or `android-biometric`.
    pub kind: String,
    pub added: u64,
}

/// A device asking to be added as a holder.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Asking {
    pub fp: String,
    pub name: String,
    pub kind: String,
    /// The six digits it shows, "123 456".
    pub code: String,
    pub made: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct HolderAnswer {
    /// The six digits to compare with the ones the adding holder shows.
    pub code: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Approver {
    pub fp: String,
    pub name: String,
    /// Answered a broadcast on this network just now.
    pub near: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SetupAnswer {
    pub recovery_code: String,
}

/// What removing a device did.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ForgetAnswer {
    /// The device was a holder, and the vault has new keys.
    #[serde(default)]
    pub rekeyed: bool,
    /// A new recovery code, to be shown once and kept: the one before opens the vault no more.
    /// Empty where the code stayed the same.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub recovery_code: String,
    /// Holders the new keys were not given to, by name, because nothing a holder sealed says
    /// they are holders: each is to be added again.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub left_out: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAnswer {
    pub session: String,
    pub entries: Vec<Summary>,
    /// A recovery code made on this device and not yet said to be saved (the window that asked
    /// for it went first): shown again until it is ([`Request::RecoverySaved`]). Only ever to a
    /// program on the device itself; empty otherwise.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub recovery_code: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListAnswer {
    pub entries: Vec<Summary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetAnswer {
    pub entry: Entry,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IdAnswer {
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TotpAnswer {
    pub code: String,
    pub remaining: u64,
    pub period: u64,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RevealAnswer {
    pub values: Vec<String>,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FillAnswer {
    pub logins: Vec<Login>,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Login {
    pub id: String,
    pub title: String,
    pub username: String,
    pub password: String,
    /// The current one-time code, when the entry has a secret for one.
    #[serde(default)]
    pub totp: Option<String>,
}

/// All base64url.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreatedAnswer {
    pub credential_id: String,
    pub authenticator_data: String,
    pub attestation_object: String,
    /// SubjectPublicKeyInfo DER, for `getPublicKey()`.
    pub public_key: String,
    pub alg: i64,
    pub transports: Vec<String>,
}

/// All base64url.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AssertedAnswer {
    pub credential_id: String,
    pub authenticator_data: String,
    pub signature: String,
    pub user_handle: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChoicesAnswer {
    pub choices: Vec<Choice>,
}

/// One account's signed answer: the caller shows the names when there are several and gives
/// the page only the one chosen.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Choice {
    pub user_name: String,
    pub user_display: String,
    #[serde(flatten)]
    pub assertion: AssertedAnswer,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TargetAnswer {
    pub window: u64,
    pub app: String,
    pub title: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct DevicesAnswer {
    /// Devices that may ask this Mac.
    pub allowed: Vec<Device>,
    /// Macs that can open the vault with their Touch ID: [`DevicesAnswer::holders`] as SpatiKey's
    /// windows from before other holders read it.
    pub touch_ids: Vec<Device>,
    /// Every device that opens the vault with its biometrics.
    #[serde(default)]
    pub holders: Vec<Holder>,
    /// Devices asking to be added to them.
    #[serde(default)]
    pub requests: Vec<Asking>,
    /// Removing a holder now would make a new recovery code too, the one there is no longer
    /// opening the vault (its record is of the kind from before 2026-11-18).
    #[serde(default)]
    pub removal_new_code: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Device {
    pub fp: String,
    pub name: String,
    pub added: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SettingsAnswer {
    pub settings: Settings,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct FindAnswer {
    pub macs: Vec<Found>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Found {
    pub fp: String,
    pub name: String,
    pub address: String,
    /// Already paired with this device.
    pub paired: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PairAnswer {
    /// The six digits both ends showed.
    pub code: String,
    pub paired: bool,
}

// ---- the envelope of an answer ----

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Code {
    /// There is no vault yet.
    NoVault,
    /// There already is one.
    VaultExists,
    /// The person said no, or cancelled the Touch ID panel.
    Denied,
    /// Touch ID cannot be used now (the lid is shut, the Mac is locked, no finger is enrolled).
    TouchIdUnavailable,
    /// The fingers enrolled changed; the recovery code sets Touch ID up again.
    FingerprintsChanged,
    /// This device has no Mac to ask: pair with one first.
    NoApprover,
    /// No paired Mac answered on this network.
    NotReachable,
    /// The Mac refused: this device is not on its network.
    NotSameLan,
    /// Nothing in the vault for that.
    NoMatch,
    /// The site already has a passkey from SpatiKey for this account (WebAuthn's
    /// InvalidStateError).
    Excluded,
    BadRequest,
    /// The session is closed, timed out, or belongs to someone else.
    BadSession,
    /// Not something this machine can do (setting up on a Deck, typing with no room).
    NotHere,
    /// The Mac did not answer in time.
    Timeout,
    Failed,
    /// The entry changed (or went) since the change being saved was begun: read it again.
    Stale,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Refusal {
    pub code: Code,
    #[serde(default)]
    pub message: String,
}

impl Refusal {
    pub fn new(code: Code, message: impl Into<String>) -> Refusal {
        Refusal { code, message: message.into() }
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.code, self.message)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reply {
    pub id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ok: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub err: Option<Refusal>,
}

impl Reply {
    pub fn ok(id: u64, answer: impl Serialize) -> Reply {
        match serde_json::to_value(answer) {
            Ok(value) => Reply { id, ok: Some(value), err: None },
            Err(e) => Reply::err(id, Refusal::new(Code::Failed, format!("could not write the answer: {e}"))),
        }
    }

    pub fn err(id: u64, refusal: Refusal) -> Reply {
        Reply { id, ok: None, err: Some(refusal) }
    }

    /// The answer, or the refusal.
    pub fn into_result(self) -> Result<Value, Refusal> {
        match (self.ok, self.err) {
            (_, Some(refusal)) => Err(refusal),
            (Some(value), None) => Ok(value),
            (None, None) => Ok(Value::Object(Default::default())),
        }
    }
}

/// Write one message.
pub fn write_frame(out: &mut impl Write, json: &[u8]) -> std::io::Result<()> {
    if json.len() > MAX_MESSAGE {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "message too large"));
    }
    out.write_all(&(json.len() as u32).to_be_bytes())?;
    out.write_all(json)?;
    out.flush()
}

/// Read one message.
pub fn read_frame(input: &mut impl Read) -> std::io::Result<Vec<u8>> {
    let mut length = [0u8; 4];
    input.read_exact(&mut length)?;
    let length = u32::from_be_bytes(length) as usize;
    if length > MAX_MESSAGE {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "message too large"));
    }
    let mut body = vec![0u8; length];
    input.read_exact(&mut body)?;
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_read_as_the_extensions_write_them() {
        let text = r#"{"id":7,"op":"passkey_get","origin":"https://github.com","rp_id":"github.com","client_data_hash":"AAAA"}"#;
        let env: Envelope = serde_json::from_str(text).unwrap();
        assert_eq!(env.id, 7);
        assert_eq!(env.who, Who::default());
        match env.request {
            Request::PasskeyGet(get) => {
                assert_eq!(get.rp_id, "github.com");
                assert!(get.allow.is_empty() && !get.conditional);
            }
            other => panic!("read as {other:?}"),
        }
        let status: Envelope = serde_json::from_str(r#"{"id":1,"op":"status","who":{"app":"Chrome"}}"#).unwrap();
        assert!(matches!(status.request, Request::Status));
        assert_eq!(status.who.app, "Chrome");
    }

    #[test]
    fn answers_carry_ok_or_err() {
        let ok = serde_json::to_string(&Reply::ok(3, IdAnswer { id: "x".into() })).unwrap();
        assert_eq!(ok, r#"{"id":3,"ok":{"id":"x"}}"#);
        let err = serde_json::to_string(&Reply::err(4, Refusal::new(Code::NotSameLan, "no"))).unwrap();
        assert_eq!(err, r#"{"id":4,"err":{"code":"not_same_lan","message":"no"}}"#);
    }

    /// Every request as [`crate::socket::Client::ask`] writes it, read back as the local socket
    /// reads it (through a `Value`, for the id of a refusal) and as the devices' channel does
    /// (straight from the bytes). Several requests have an `id` of their own beside the
    /// envelope's: neither may take the other's.
    #[test]
    fn every_request_survives_the_envelope() {
        let s = || "s".to_string();
        let entry = Entry { id: "e1".into(), title: "t".into(), password: "pw".into(), ..Entry::default() };
        let requests = vec![
            Request::Status,
            Request::Setup { name: "n".into() },
            Request::Open,
            Request::Close { session: s() },
            Request::List { session: s() },
            Request::Get { session: s(), id: "e1".into() },
            Request::Put { session: s(), entry: entry.clone(), based_on: None },
            Request::Put { session: s(), entry, based_on: Some(17) },
            Request::Delete { session: s(), id: "e1".into() },
            Request::Totp { session: Some(s()), id: "e1".into() },
            Request::Totp { session: None, id: "e1".into() },
            Request::Reveal { session: None, id: "e1".into(), fields: vec![Field::Password] },
            Request::Fill { origin: "https://a.b".into() },
            Request::Save { origin: "https://a.b".into(), title: "t".into(), username: "u".into(), password: "p".into() },
            Request::ProfileUnlock { profile: "p".into() },
            Request::PasskeyCreate(PasskeyCreate {
                origin: "https://a.b".into(),
                rp_id: "a.b".into(),
                rp_name: String::new(),
                user_id: "AA".into(),
                user_name: "u".into(),
                user_display: String::new(),
                client_data_hash: "AA".into(),
                exclude: vec![],
                algs: vec![-7],
            }),
            Request::PasskeyGet(PasskeyGet {
                origin: "https://a.b".into(),
                rp_id: "a.b".into(),
                client_data_hash: "AA".into(),
                allow: vec![],
                conditional: false,
            }),
            Request::Type { session: None, id: "e1".into(), fields: vec![Field::Username, Field::Tab], window: Some(3) },
            Request::Target,
            Request::RecoveryNew { session: s() },
            Request::Recover { code: "c".into() },
            Request::RecoverySaved { code: "c".into() },
            Request::Devices { session: s() },
            Request::ForgetDevice { session: s(), fp: "f".into(), recovery_code: None },
            Request::ForgetDevice { session: s(), fp: "f".into(), recovery_code: Some("c".into()) },
            Request::Settings { session: s(), set: Some(Settings::default()) },
            Request::Pair { name: "n".into() },
            Request::Find,
            Request::PairWith { fp: "f".into() },
            Request::PairConfirm { fp: "f".into(), ok: true },
            Request::HolderRequest { name: "n".into() },
            Request::HolderApprove { session: s(), fp: "f".into(), code: "123456".into() },
            Request::HolderRefuse { session: s(), fp: "f".into() },
        ];
        for request in requests {
            let shown = format!("{request:?}");
            let env = Envelope { id: 41, who: Who { app: "SpatiKey".into(), device: String::new() }, request };
            let json = serde_json::to_vec(&env).unwrap();
            let text = String::from_utf8_lossy(&json).to_string();
            let value: Value = serde_json::from_slice(&json).unwrap_or_else(|e| panic!("{text}: not JSON: {e}"));
            assert_eq!(id_of(&json), 41, "{text}: the refusal's id");
            let local: Envelope = serde_json::from_value(value).unwrap_or_else(|e| panic!("{text}: through a value: {e}"));
            let remote: Envelope = serde_json::from_slice(&json).unwrap_or_else(|e| panic!("{text}: from the bytes: {e}"));
            for read in [local, remote] {
                assert_eq!((read.id, read.who.app.as_str()), (41, "SpatiKey"), "{text}");
                assert_eq!(format!("{:?}", read.request), shown, "{text}");
            }
        }
    }

    /// What builds before 2026-11-18 wrote for the requests naming an entry: the envelope's id and
    /// the entry's, both as `id`. Read, so that a newer service answers an older window or device.
    #[test]
    fn the_older_two_ids_are_read() {
        let old = br#"{"id":41,"who":{"app":"SpatiKey","device":""},"op":"get","session":"s","id":"e1"}"#;
        assert_eq!(id_of(old), 41);
        let env: Envelope = serde_json::from_slice(old).unwrap();
        assert_eq!(env.id, 41);
        assert!(matches!(env.request, Request::Get { ref session, ref id } if session == "s" && id == "e1"));
        // The entry's id first is read the same.
        let first: Envelope = serde_json::from_slice(br#"{"op":"delete","id":"e2","session":"s","id":7}"#).unwrap();
        assert!(matches!(first.request, Request::Delete { ref id, .. } if id == "e2") && first.id == 7);
        // Two envelope ids, or none, is still not a request.
        assert!(serde_json::from_slice::<Envelope>(br#"{"id":1,"id":2,"op":"status"}"#).is_err());
        assert!(serde_json::from_slice::<Envelope>(br#"{"op":"status"}"#).is_err());
        assert!(serde_json::from_slice::<Envelope>(br#"{"id":1,"op":"get","session":"s","entry":"a","id":"b"}"#).is_err());
        assert_eq!(id_of(b"not json"), 0);
        assert_eq!(id_of(br#"{"op":"get","id":"e1"}"#), 0);
    }

    #[test]
    fn frames_round_trip() {
        let mut buffer = Vec::new();
        write_frame(&mut buffer, b"{}").unwrap();
        assert_eq!(buffer, [0, 0, 0, 2, b'{', b'}']);
        assert_eq!(read_frame(&mut buffer.as_slice()).unwrap(), b"{}");
    }
}
