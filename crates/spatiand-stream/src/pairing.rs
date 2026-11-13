//! Pairing: a code shown at the host, typed on the device, and proved without being sent.
//!
//! Pairing is the one step that decides who is trusted, and what it trusts is a lot: a paired
//! device is written into the owner's roster, and the person's key and data follow it there. So
//! it must not be enough to connect to a host at the right moment.
//!
//! ## The scheme
//!
//! 1. The host opens a [`Window`]: a fresh random six-digit code, shown **only at the host** --
//!    `spatiand-host --pair` prints it, the Mac shows it in its pairing panel. It never crosses
//!    the network, and nothing asks anybody at the host to say yes.
//! 2. The person types it on the device. The device never shows a code of its own.
//! 3. Both ends run SPAKE2 (Ed25519 group) with the code as the password and the two TLS
//!    certificate fingerprints as the identities -- the device's as A, the host's as B. The
//!    device says [`ClientMessage::PairStart`]; the host answers [`HostMessage::PairReply`] with
//!    its half and its key confirmation; the device checks that (a wrong code ends it there,
//!    with nothing written down) and says [`ClientMessage::PairConfirm`] with its own; the host
//!    checks that, and only then writes the device down and lets it in.
//!
//! What this buys:
//!
//! * **Being first is not enough.** Somebody who connects while the window is open, without
//!   the code, gets one guess -- one in a million -- per attempt, and a window allows
//!   [`ATTEMPTS`] attempts before it closes itself.
//! * **Nothing to overhear.** What crosses the wire is two curve points and two MACs; neither
//!   says anything about the code to somebody watching, and checking a guess against them
//!   needs a live exchange with one of the ends.
//! * **A machine in the middle fails too.** It must present its own certificate to each side,
//!   so the fingerprints bound into each leg differ, the keys differ, and it cannot relay one
//!   leg's messages into the other: it is reduced to guessing the code, once.
//!
//! An attempt is spent when the host answers a [`ClientMessage::PairStart`], not when a wrong
//! confirmation arrives: the reply already carries the host's confirmation, which lets whoever
//! asked check one guess at leisure, so the attempt is used whether or not it comes back. A
//! `PairStart` that cannot be answered -- its bytes are not a SPAKE2 message -- spends nothing.
//!
//! Both hosts -- the Linux `spatiand-host` and the Mac's -- run [`Window`]; every device runs
//! [`Prover`] (through `link::pair`). One implementation, so the two cannot drift apart.

use std::time::{Duration, Instant};

use ring::hmac;
use ring::rand::{SecureRandom, SystemRandom};
use spake2::{Ed25519Group, Identity as SpakeIdentity, Password, Spake2};

use crate::transport::Fingerprint;
use crate::{ClientMessage, HostMessage};

/// How many attempts one window allows before it closes itself.
pub const ATTEMPTS: u32 = 3;

/// How long a window stays open when nobody pairs through it. Long enough to put a headset on.
pub const WINDOW: Duration = Duration::from_secs(120);

/// How many digits a code has.
pub const DIGITS: usize = 6;

/// What a device is told when the code it was given was not the host's.
pub const WRONG_CODE: &str = "the code was not right";

/// The labels the two confirmations are made under: different, so one cannot be sent back as
/// the other.
const HOST_LABEL: &[u8] = b"spatiand pairing v2 host confirms";
const CLIENT_LABEL: &[u8] = b"spatiand pairing v2 device confirms";

/// A fresh code: six decimal digits, uniformly drawn from the system's random source.
pub fn fresh_code() -> Result<String, String> {
    let random = SystemRandom::new();
    // The largest multiple of a million below 2^32: a draw above it is drawn again, so every
    // code is exactly as likely as every other.
    const LIMIT: u32 = 4_294_000_000;
    loop {
        let mut bytes = [0u8; 4];
        random
            .fill(&mut bytes)
            .map_err(|_| "this machine has no random numbers to make a pairing code with".to_string())?;
        let n = u32::from_le_bytes(bytes);
        if n < LIMIT {
            return Ok(format!("{:06}", n % 1_000_000));
        }
    }
}

/// A code as a person reads it: `482 913`.
pub fn show(code: &str) -> String {
    if code.len() == DIGITS && code.is_ascii() {
        format!("{} {}", &code[..3], &code[3..])
    } else {
        code.to_string()
    }
}

/// What somebody typed, as a code: its digits, if there are exactly six of them. Spaces and
/// dashes are what people put between groups, so they are allowed and dropped; anything else
/// makes it not a code.
pub fn typed(text: &str) -> Option<String> {
    let mut digits = String::new();
    for c in text.chars() {
        match c {
            '0'..='9' => digits.push(c),
            ' ' | '-' | '\u{2009}' | '\u{a0}' => {}
            _ => return None,
        }
    }
    (digits.len() == DIGITS).then_some(digits)
}

fn spake_identities(client: &Fingerprint, host: &Fingerprint) -> (SpakeIdentity, SpakeIdentity) {
    (SpakeIdentity::new(&client.0), SpakeIdentity::new(&host.0))
}

fn confirmation(key: &[u8], label: &[u8], client: &Fingerprint, host: &Fingerprint) -> hmac::Tag {
    let key = hmac::Key::new(hmac::HMAC_SHA256, key);
    let mut context = hmac::Context::with_key(&key);
    context.update(label);
    context.update(&client.0);
    context.update(&host.0);
    context.sign()
}

/// Whether `tag` is the confirmation `key` makes under `label`, compared in constant time.
fn confirms(key: &[u8], label: &[u8], client: &Fingerprint, host: &Fingerprint, tag: &[u8]) -> bool {
    let key = hmac::Key::new(hmac::HMAC_SHA256, key);
    let mut message = Vec::with_capacity(label.len() + 64);
    message.extend_from_slice(label);
    message.extend_from_slice(&client.0);
    message.extend_from_slice(&host.0);
    hmac::verify(&key, &message, tag).is_ok()
}

/// How a device's confirmation was taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// The right code: write this device down and let it in. The window is spent.
    Paired(Fingerprint),
    /// Not the right code, and this many attempts are left.
    Wrong { left: u32 },
    /// Not the right code, and that was the last attempt: the window is closed.
    Locked,
    /// A confirmation from somebody who has not started, or is not the one who did. Ignored,
    /// and costs nothing: it proves nothing either way.
    Stray,
}

/// The host's side: one pairing window, with its code, its clock and its attempts.
pub struct Window {
    code: String,
    until: Instant,
    started: u32,
    /// The exchange under way, if one is.
    pending: Option<HostExchange>,
    paired: bool,
}

struct HostExchange {
    client: Fingerprint,
    host: Fingerprint,
    key: Vec<u8>,
}

impl Window {
    /// Open a window with a fresh code, for `lasts`.
    pub fn open(lasts: Duration) -> Result<Window, String> {
        Ok(Window::with_code(fresh_code()?, lasts))
    }

    /// A window with a code chosen by the caller. Only tests have a reason to choose one.
    pub fn with_code(code: String, lasts: Duration) -> Window {
        Window {
            code,
            until: Instant::now() + lasts,
            started: 0,
            pending: None,
            paired: false,
        }
    }

    /// The code, for the host to show to the person at it. Never for the network.
    pub fn code(&self) -> &str {
        &self.code
    }

    /// How long until it closes on its own.
    pub fn remaining(&self) -> Duration {
        self.until.saturating_duration_since(Instant::now())
    }

    /// Its time is up.
    pub fn expired(&self) -> bool {
        Instant::now() >= self.until
    }

    /// Attempts not yet started.
    pub fn attempts_left(&self) -> u32 {
        ATTEMPTS.saturating_sub(self.started)
    }

    /// Every attempt has been spent and none is under way: it is closed for good.
    pub fn locked(&self) -> bool {
        self.started >= ATTEMPTS && self.pending.is_none() && !self.paired
    }

    /// Whether `client` is part way through an exchange.
    pub fn is_pending(&self, client: &Fingerprint) -> bool {
        self.pending.as_ref().is_some_and(|p| p.client == *client)
    }

    /// A device's [`ClientMessage::PairStart`]. Spends an attempt, and gives the reply to send.
    ///
    /// `host` is this host's own fingerprint, `client` the one the device's certificate has.
    /// An exchange already under way is abandoned by a new one: its attempt stays spent.
    pub fn start(&mut self, host: Fingerprint, client: Fingerprint, spake: &[u8]) -> Result<HostMessage, String> {
        if self.paired {
            return Err("this host has just paired with another device".into());
        }
        if self.expired() {
            return Err("pairing on this host has closed again".into());
        }
        if self.started >= ATTEMPTS {
            self.pending = None;
            return Err("too many wrong codes; pairing on this host has closed".into());
        }
        let (a, b) = spake_identities(&client, &host);
        let (state, mine) = Spake2::<Ed25519Group>::start_b(&Password::new(self.code.as_bytes()), &a, &b);
        // **Before anything is spent.** A message that is not one is answered with nothing, so
        // whoever sent it has learnt nothing to check a guess with, and it costs the window
        // nothing: three of them used to close it, with every attempt gone and nobody at the
        // host told, and whatever exchange was under way was dropped on the way.
        let key = state
            .finish(spake)
            .map_err(|e| format!("the device's pairing message is not one ({e:?})"))?;
        self.pending = None;
        self.started += 1;
        let confirm = confirmation(&key, HOST_LABEL, &client, &host).as_ref().to_vec();
        self.pending = Some(HostExchange { client, host, key });
        Ok(HostMessage::PairReply { spake: mine, confirm })
    }

    /// A device's [`ClientMessage::PairConfirm`].
    pub fn confirm(&mut self, client: Fingerprint, confirm: &[u8]) -> Verdict {
        let Some(exchange) = self.pending.as_ref() else { return Verdict::Stray };
        if exchange.client != client {
            return Verdict::Stray;
        }
        let exchange = self.pending.take().expect("checked above");
        if confirms(&exchange.key, CLIENT_LABEL, &exchange.client, &exchange.host, confirm) {
            self.paired = true;
            return Verdict::Paired(exchange.client);
        }
        self.failed()
    }

    /// The device part way through an exchange went away without finishing it. Its attempt
    /// stays spent; what that leaves is said as a confirmation would have said it. `None` when
    /// it had nothing under way.
    pub fn abandon(&mut self, client: &Fingerprint) -> Option<Verdict> {
        if !self.is_pending(client) {
            return None;
        }
        self.pending = None;
        Some(self.failed())
    }

    fn failed(&self) -> Verdict {
        if self.started >= ATTEMPTS {
            Verdict::Locked
        } else {
            Verdict::Wrong { left: self.attempts_left() }
        }
    }
}

/// The device's side of one attempt.
pub struct Prover {
    state: Spake2<Ed25519Group>,
    client: Fingerprint,
    host: Fingerprint,
}

impl Prover {
    /// Begin, with the code as the person typed it (see [`typed`]), this device's fingerprint
    /// and the one the host's certificate has. Gives the first message to send.
    pub fn start(code: &str, client: Fingerprint, host: Fingerprint) -> (Prover, ClientMessage) {
        let (a, b) = spake_identities(&client, &host);
        let (state, spake) = Spake2::<Ed25519Group>::start_a(&Password::new(code.as_bytes()), &a, &b);
        (Prover { state, client, host }, ClientMessage::PairStart { spake })
    }

    /// The host's [`HostMessage::PairReply`]. If its confirmation holds -- the host knows the
    /// same code, and is the host whose certificate this connection has -- gives this device's
    /// confirmation to send. Otherwise the code was not right, and nothing may be written down.
    pub fn finish(self, spake: &[u8], confirm: &[u8]) -> Result<ClientMessage, String> {
        let key = self
            .state
            .finish(spake)
            .map_err(|_| WRONG_CODE.to_string())?;
        if !confirms(&key, HOST_LABEL, &self.client, &self.host, confirm) {
            return Err(WRONG_CODE.into());
        }
        let mine = confirmation(&key, CLIENT_LABEL, &self.client, &self.host);
        Ok(ClientMessage::PairConfirm { confirm: mine.as_ref().to_vec() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fp(n: u8) -> Fingerprint {
        Fingerprint([n; 32])
    }

    const LASTS: Duration = Duration::from_secs(120);

    /// One whole exchange, in memory: the device with `typed` as its code and the fingerprints
    /// it sees, the host with the ones it sees.
    fn attempt(
        window: &mut Window,
        typed: &str,
        device_sees: (Fingerprint, Fingerprint),
        host_sees: (Fingerprint, Fingerprint),
    ) -> (Result<(), String>, Option<Verdict>) {
        let (prover, start) = Prover::start(typed, device_sees.0, device_sees.1);
        let ClientMessage::PairStart { spake } = start else { unreachable!() };
        let reply = match window.start(host_sees.1, host_sees.0, &spake) {
            Ok(reply) => reply,
            Err(e) => return (Err(e), None),
        };
        let HostMessage::PairReply { spake, confirm } = reply else { unreachable!() };
        match prover.finish(&spake, &confirm) {
            Ok(ClientMessage::PairConfirm { confirm }) => (Ok(()), Some(window.confirm(host_sees.0, &confirm))),
            Ok(_) => unreachable!(),
            // The device stops there and hangs up; the host hears it go.
            Err(e) => (Err(e), window.abandon(&host_sees.0)),
        }
    }

    #[test]
    fn codes_are_six_digits_and_differ() {
        let a = fresh_code().unwrap();
        assert_eq!(a.len(), DIGITS);
        assert!(a.chars().all(|c| c.is_ascii_digit()));
        let many: std::collections::HashSet<String> = (0..50).map(|_| fresh_code().unwrap()).collect();
        assert!(many.len() > 45, "fifty fresh codes are not all the same few");
        assert_eq!(show("482913"), "482 913");
    }

    #[test]
    fn a_typed_code_is_its_six_digits() {
        assert_eq!(typed("482 913").as_deref(), Some("482913"));
        assert_eq!(typed("482-913").as_deref(), Some("482913"));
        assert_eq!(typed(" 482913 ").as_deref(), Some("482913"));
        assert_eq!(typed("48291"), None);
        assert_eq!(typed("4829134"), None);
        assert_eq!(typed("48291x"), None);
    }

    #[test]
    fn the_right_code_pairs() {
        let (device, host) = (fp(1), fp(2));
        let mut window = Window::with_code("482913".into(), LASTS);
        let (seen, verdict) = attempt(&mut window, "482913", (device, host), (device, host));
        assert_eq!(seen, Ok(()));
        assert_eq!(verdict, Some(Verdict::Paired(device)));
        // And the window is spent: nobody pairs on the back of it.
        assert!(window.start(host, fp(3), &[0; 33]).is_err());
    }

    #[test]
    fn a_wrong_code_is_refused_on_the_device_and_admits_nothing() {
        let (device, host) = (fp(1), fp(2));
        let mut window = Window::with_code("482913".into(), LASTS);
        let (seen, verdict) = attempt(&mut window, "482914", (device, host), (device, host));
        assert_eq!(seen, Err(WRONG_CODE.to_string()), "the device finds out first, and writes nothing");
        assert_eq!(verdict, Some(Verdict::Wrong { left: 2 }));
        assert!(!window.locked());
    }

    #[test]
    fn a_forged_confirmation_is_wrong_on_the_host() {
        let (device, host) = (fp(1), fp(2));
        let mut window = Window::with_code("482913".into(), LASTS);
        // Somebody without the code, who skips checking the host's reply and makes up its own.
        let (_, start) = Prover::start("000000", device, host);
        let ClientMessage::PairStart { spake } = start else { unreachable!() };
        window.start(host, device, &spake).unwrap();
        assert_eq!(window.confirm(device, &[0u8; 32]), Verdict::Wrong { left: 2 });
        // A confirmation nobody asked for costs nothing, and proves nothing.
        assert_eq!(window.confirm(device, &[0u8; 32]), Verdict::Stray);
    }

    #[test]
    fn three_wrong_attempts_close_the_window() {
        let (device, host) = (fp(1), fp(2));
        let mut window = Window::with_code("482913".into(), LASTS);
        assert_eq!(attempt(&mut window, "111111", (device, host), (device, host)).1, Some(Verdict::Wrong { left: 2 }));
        assert_eq!(attempt(&mut window, "222222", (device, host), (device, host)).1, Some(Verdict::Wrong { left: 1 }));
        assert_eq!(attempt(&mut window, "333333", (device, host), (device, host)).1, Some(Verdict::Locked));
        assert!(window.locked());
        // Not even the right code gets in now.
        let (seen, verdict) = attempt(&mut window, "482913", (device, host), (device, host));
        assert!(seen.unwrap_err().contains("too many"));
        assert_eq!(verdict, None);
    }

    #[test]
    fn an_attempt_abandoned_after_the_reply_is_spent() {
        let (device, host) = (fp(1), fp(2));
        let mut window = Window::with_code("482913".into(), LASTS);
        for _ in 0..ATTEMPTS {
            let (_, start) = Prover::start("000000", device, host);
            let ClientMessage::PairStart { spake } = start else { unreachable!() };
            // Asked, heard the reply (and with it a way to check one guess), and left.
            window.start(host, device, &spake).unwrap();
        }
        assert_eq!(window.abandon(&device), Some(Verdict::Locked));
        assert!(window.locked());
    }

    #[test]
    fn a_message_that_is_not_one_spends_nothing() {
        let (device, host) = (fp(1), fp(2));
        let mut window = Window::with_code("482913".into(), LASTS);
        // Somebody part way through, honestly.
        let (prover, start) = Prover::start("482913", device, host);
        let ClientMessage::PairStart { spake } = start else { unreachable!() };
        let HostMessage::PairReply { spake, confirm } = window.start(host, device, &spake).unwrap() else {
            unreachable!()
        };
        assert_eq!(window.attempts_left(), ATTEMPTS - 1);
        // Too short, too long, the right length and not a point with the right side's mark.
        for nonsense in [&[][..], &[0u8; 5][..], &[0u8; 33][..], &[0xffu8; 33][..], &[0u8; 64][..]] {
            for _ in 0..ATTEMPTS + 1 {
                assert!(window.start(host, fp(7), nonsense).is_err());
            }
        }
        assert_eq!(window.attempts_left(), ATTEMPTS - 1, "nothing was answered, so nothing is spent");
        assert!(!window.locked());
        // And the exchange under way is still under way: it finishes and pairs.
        let Ok(ClientMessage::PairConfirm { confirm }) = prover.finish(&spake, &confirm) else { unreachable!() };
        assert_eq!(window.confirm(device, &confirm), Verdict::Paired(device));
    }

    #[test]
    fn a_machine_in_the_middle_fails_even_with_the_right_code() {
        // The device reached the middle's certificate; the host was reached by the middle's
        // other certificate. Each leg is honest about what it sees, and the code is right.
        let (device, host) = (fp(1), fp(2));
        let (middle_as_host, middle_as_device) = (fp(8), fp(9));
        let mut window = Window::with_code("482913".into(), LASTS);

        // The middle relays the device's messages to the host untouched.
        let (prover, start) = Prover::start("482913", device, middle_as_host);
        let ClientMessage::PairStart { spake } = start else { unreachable!() };
        let HostMessage::PairReply { spake, confirm } = window.start(host, middle_as_device, &spake).unwrap() else {
            unreachable!()
        };
        assert_eq!(prover.finish(&spake, &confirm), Err(WRONG_CODE.to_string()), "the device sees through it");
        assert_eq!(window.abandon(&middle_as_device), Some(Verdict::Wrong { left: 2 }), "and the host admits nothing");
    }

    #[test]
    fn a_closed_window_starts_nothing() {
        let mut window = Window::with_code("482913".into(), Duration::ZERO);
        assert!(window.expired());
        let (_, start) = Prover::start("482913", fp(1), fp(2));
        let ClientMessage::PairStart { spake } = start else { unreachable!() };
        assert!(window.start(fp(2), fp(1), &spake).is_err());
    }
}
