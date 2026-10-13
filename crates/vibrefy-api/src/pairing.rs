//! Signing in with eight digits.
//!
//! Ported from `data/Pairing.kt` and the pairing half of `data/VibrefyJson.kt`. This is
//! the *only* way in on Linux: Vibrefy's other route is OpenID in a browser, and there is
//! no embedded browser here to keep the cookie it would set. `CLIENT-API.md` describes
//! both directions, and both end with this session signed in and its cookie rotated.

use serde_json::Value;

/// Where a pairing has got to. One shape covers both directions - the poll a waiting
/// device makes, and the answer to a code typed in here - because the server answers both
/// from the same `getJson`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairingState {
    /// Still nobody has approved it. `seconds_left` is the server's own countdown.
    Waiting { seconds_left: u64 },
    /// Approved, and this session is now signed in as `account`.
    Approved { account: Option<String> },
    /// The three minutes ran out. A new code has to be asked for.
    Expired,
    /// The server turned the code down and said why - wrong, expired, or too many guesses
    /// from this address. Its wording is passed through: the throttle in particular is
    /// something the user needs told, not a generic failure.
    Refused { reason: String },
}

/// A freshly minted code to put on screen, with the server's own life left on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingOffer {
    pub code: String,
    pub seconds_left: u64,
}

/// The shape of a pairing code, wherever one is typed, shown or checked.
pub struct PairingCode;

impl PairingCode {
    pub const LENGTH: usize = 8;

    /// Eight digits, shown as two groups so they can be read off a distant screen.
    pub fn format(code: &str) -> String {
        if code.len() == Self::LENGTH {
            format!("{} {}", &code[..Self::LENGTH / 2], &code[Self::LENGTH / 2..])
        } else {
            code.to_string()
        }
    }
}

/// A sign-in link the server will honour: the key this client generated and the address
/// to open a browser at.
///
/// The other direction of the same pairing. Eight digits exist to be read off a screen and
/// typed on another device; a link exists because the *other device can be this one* - the
/// browser on this machine signs in with Google, and one click there approves the pairing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingLink {
    /// What to poll with. Not shown to anybody: it is a secret, and a long one.
    pub key: String,
    /// Where to send a browser. The server's own wording of it, including its public host
    /// name - which is not always the address this client was pointed at.
    pub url: String,
    pub seconds_left: u64,
}

/// The key a client generates for a sign-in link.
///
/// Generated here rather than asked for, because the point of the link is that the address
/// exists *before* the browser is opened. `Pairing.isLinkKey` bounds what the server will
/// take: 32 to 96 characters from the URL-safe base64 alphabet, so a key drops into a path
/// segment with nothing to percent-encode.
pub struct LinkKey;

impl LinkKey {
    /// 43 characters, which is 32 bytes of randomness in base64 - the same strength as a
    /// session id and eleven orders of magnitude past guessing. Eight digits can be brute
    /// forced in principle and the server throttles them for that reason; this cannot, and
    /// that is what lets a link be approved with one click instead of by someone
    /// deliberately copying digits across.
    pub const LENGTH: usize = 43;

    /// Reads the system's own entropy. `/dev/urandom` rather than a crate: this is a Linux
    /// client, the file is the kernel's answer to exactly this question, and a
    /// dependency's worth of code to open it would still open it.
    pub fn generate() -> std::io::Result<String> {
        use std::io::Read as _;
        let mut bytes = [0u8; 32];
        std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
        Ok(encode(&bytes))
    }
}

/// URL-safe base64 without padding, which is the alphabet `Pairing.isLinkKey` accepts.
fn encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let mut block = 0u32;
        for (at, byte) in chunk.iter().enumerate() {
            block |= (*byte as u32) << (16 - 8 * at);
        }
        // One character per six bits, and only as many as the bytes actually filled: three
        // bytes are four characters, two are three, one is two. The padding '=' is left off
        // because it is not in the alphabet the server will accept.
        for at in 0..chunk.len() + 1 {
            out.push(ALPHABET[((block >> (18 - 6 * at)) & 0x3f) as usize] as char);
        }
    }
    out
}

pub const REFUSED_DEFAULT: &str = "That code is not valid, or it has expired.";

/// What `PairBean` says a pairing has come to.
pub fn parse(json: &Value) -> PairingState {
    let message = json
        .get("message")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty());
    // A signed-out answer carries a real JSON null here, which is "nobody" and not a name.
    let account = json
        .get("account")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    match json.get("state").and_then(Value::as_str).unwrap_or_default() {
        "waiting" => PairingState::Waiting {
            seconds_left: json.get("seconds").and_then(Value::as_u64).unwrap_or(0),
        },
        "done" => PairingState::Approved { account },
        "expired" => PairingState::Expired,
        // "invalid" carries the server's own wording; anything else is a state this
        // direction of the flow never asked for, and is no more usable than a refusal.
        _ => PairingState::Refused {
            reason: message.unwrap_or(REFUSED_DEFAULT).to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairing(json: &str) -> PairingState {
        parse(&serde_json::from_str(json).expect("valid fixture"))
    }

    #[test]
    fn a_minted_code_comes_back_waiting_with_the_life_the_server_gave_it() {
        assert_eq!(
            PairingState::Waiting { seconds_left: 177 },
            pairing(r#"{"state":"waiting","code":"48219037","seconds":177,"account":null}"#)
        );
    }

    #[test]
    fn an_approval_names_the_account_it_signed_this_device_in_as() {
        assert_eq!(
            PairingState::Approved {
                account: Some("someone@example.com".into())
            },
            pairing(r#"{"state":"done","account":"someone@example.com"}"#)
        );
    }

    /// The countdown is gone as well as the state, which is what the old HTML parse keyed
    /// on. Now that the server says "expired" outright, the two cannot disagree.
    #[test]
    fn an_expired_code_says_so_rather_than_going_quiet() {
        assert_eq!(
            PairingState::Expired,
            pairing(r#"{"state":"expired","account":null}"#)
        );
    }

    #[test]
    fn a_rejected_code_carries_the_servers_own_wording() {
        assert_eq!(
            PairingState::Refused {
                reason: "Too many incorrect codes. Wait a few minutes and try again.".into()
            },
            pairing(
                r#"{"state":"invalid","message":"Too many incorrect codes. Wait a few minutes and try again.","account":null}"#
            )
        );
    }

    /// "idle" answers an order this direction never sent; it is no more usable than a no.
    #[test]
    fn a_state_this_flow_did_not_ask_for_is_not_mistaken_for_progress() {
        assert_eq!(
            PairingState::Refused {
                reason: REFUSED_DEFAULT.into()
            },
            pairing(r#"{"state":"idle","account":null}"#)
        );
    }

    /// A JSON null account has to read as "nobody". The Android port had to rule out the
    /// four letters "null" because `optString` stringifies the sentinel; serde_json gives
    /// a real null, so this asserts the two platforms still agree.
    #[test]
    fn a_signed_out_answer_names_nobody() {
        assert_eq!(
            PairingState::Approved { account: None },
            pairing(r#"{"state":"done","account":null}"#)
        );
    }


    /// The alphabet is the server's, and the length is what 32 bytes come to. A key outside
    /// either would be refused by `Pairing.isLinkKey` - with a message about the charset
    /// that the person reading it did nothing to deserve.
    #[test]
    fn a_generated_key_is_the_shape_the_server_will_take() {
        let key = LinkKey::generate().expect("the kernel has entropy");
        assert_eq!(LinkKey::LENGTH, key.len());
        assert!(
            key.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
            "{key} is not URL-safe base64"
        );
    }

    /// Two keys in a row being equal would mean the entropy is not entropy.
    #[test]
    fn two_keys_are_not_the_same_key() {
        assert_ne!(
            LinkKey::generate().expect("entropy"),
            LinkKey::generate().expect("entropy")
        );
    }

    /// Pinned against a known vector rather than round-tripped, because the only decoder
    /// that matters is the server's and it is not here to ask.
    #[test]
    fn the_encoding_is_url_safe_base64_without_padding() {
        assert_eq!("AAAA", encode(&[0, 0, 0]));
        assert_eq!("____", encode(&[0xff, 0xff, 0xff]));
        assert_eq!("-w", encode(&[0xfb]));
        assert_eq!("SGVsbG8", encode(b"Hello"));
    }
    #[test]
    fn codes_are_shown_in_two_groups_and_anything_else_is_left_alone() {
        assert_eq!("4821 9037", PairingCode::format("48219037"));
        assert_eq!("123", PairingCode::format("123"));
    }
}
