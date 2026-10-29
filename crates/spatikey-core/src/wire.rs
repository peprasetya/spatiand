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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    pub id: u64,
    /// Who is asking, as the end that received it names them. A caller may fill in `app`; the
    /// receiving end adds what it knows itself (the peer process, the paired device's name).
    #[serde(default)]
    pub who: Who,
    #[serde(flatten)]
    pub request: Request,
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
        id: String,
    },
    /// Add (empty id) or change an entry. Answers [`IdAnswer`].
    Put {
        session: String,
        entry: Entry,
    },
    Delete {
        session: String,
        id: String,
    },
    /// The current code. With no session, asks for a touch. Answers [`TotpAnswer`].
    Totp {
        #[serde(default)]
        session: Option<String>,
        id: String,
    },
    /// Fields of one entry, for typing them. With no session, asks for a touch.
    /// Answers [`RevealAnswer`], in the order asked (`tab` and `enter` give empty strings).
    Reveal {
        #[serde(default)]
        session: Option<String>,
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
    /// Answers [`CreatedAnswer`].
    PasskeyCreate(PasskeyCreate),
    /// Answers [`AssertedAnswer`], or the error `no_match`.
    PasskeyGet(PasskeyGet),
    /// Type an entry's fields into a window (by default [`Request::Target`]'s). Answers `{}`.
    Type {
        #[serde(default)]
        session: Option<String>,
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
    /// Answers [`DevicesAnswer`].
    Devices {
        session: String,
    },
    ForgetDevice {
        session: String,
        fp: String,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAnswer {
    pub session: String,
    pub entries: Vec<Summary>,
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
pub struct TargetAnswer {
    pub window: u64,
    pub app: String,
    pub title: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct DevicesAnswer {
    /// Devices that may ask this Mac.
    pub allowed: Vec<Device>,
    /// Macs that can open the vault with their Touch ID.
    pub touch_ids: Vec<Device>,
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
    BadRequest,
    /// The session is closed, timed out, or belongs to someone else.
    BadSession,
    /// Not something this machine can do (setting up on a Deck, typing with no room).
    NotHere,
    /// The Mac did not answer in time.
    Timeout,
    Failed,
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

    #[test]
    fn frames_round_trip() {
        let mut buffer = Vec::new();
        write_frame(&mut buffer, b"{}").unwrap();
        assert_eq!(buffer, [0, 0, 0, 2, b'{', b'}']);
        assert_eq!(read_frame(&mut buffer.as_slice()).unwrap(), b"{}");
    }
}
