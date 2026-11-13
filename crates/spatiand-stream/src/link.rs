//! The session's end of a connection: saying things, hearing things, and pairing.
//!
//! Framing lives here rather than in either program, because both ends have to agree on it
//! byte for byte and a second copy is how they stop agreeing.
//!
//! * The session says one [`ClientMessage`] per unidirectional stream. A malformed message can
//!   never desynchronise the next one, and QUIC streams cost nothing to open.
//! * The host says everything on one unidirectional stream it opens first, each message
//!   prefixed with its length as a little-endian `u32`, because ordering between them matters:
//!   a window's `Stream` must not overtake its `Opened`.

use std::net::SocketAddr;
use std::time::Duration;

use crate::transport::{client_first_contact, peer_fingerprint, Fingerprint, Identity};
use crate::{ClientMessage, HostMessage};

/// Where a host listens unless told otherwise.
///
/// Nothing standard lives here, and it is deliberately not one of the ports a game streamer
/// already uses: a host and a Sunshine can run on the same machine.
pub const DEFAULT_PORT: u16 = 47600;

/// The largest control message believed, by either end.
///
/// The big one is a paste: `Clipboard::Data` carries one whole form of the clipboard, which is
/// allowed to be [`crate::clipboard::LARGEST`], and around it the form's name and the message's
/// own few bytes. So the limit follows from that one, in one place. It was three numbers that
/// each looked sensible where they stood -- 32 MiB for what may be copied, 8 MiB here, 1 MiB
/// where the host listens -- and a copied picture between the smaller and the larger of them
/// was sent whole and then taken for a stream that had lost its place: whoever received it
/// stopped listening on the control stream for good, with the link still up.
pub const MAX_CONTROL: usize = crate::clipboard::LARGEST + (1 << 20);

/// Say one thing to the host.
pub async fn say(connection: &quinn::Connection, message: &ClientMessage) -> Result<(), String> {
    let bytes = crate::to_bytes(message).map_err(|e| format!("could not encode: {e}"))?;
    let mut stream = connection
        .open_uni()
        .await
        .map_err(|e| format!("the link is gone: {e}"))?;
    stream
        .write_all(&bytes)
        .await
        .map_err(|e| format!("the link is gone: {e}"))?;
    let _ = stream.finish();
    Ok(())
}

/// Read one message from the host's control stream, and hand the stream back for the next.
///
/// `None` is the stream ending, for whatever reason: the host closing, the link dropping, or a
/// length that cannot be trusted, which leaves nothing after it that can be. A message whole by
/// its length that this session does not know -- a newer host's -- is skipped.
pub async fn hear(mut stream: quinn::RecvStream) -> Option<(HostMessage, quinn::RecvStream)> {
    loop {
        let mut length = [0u8; 4];
        stream.read_exact(&mut length).await.ok()?;
        let length = u32::from_le_bytes(length) as usize;
        if length > MAX_CONTROL {
            log::warn!("a control message claiming {length} bytes is not one");
            return None;
        }
        let mut bytes = vec![0u8; length];
        stream.read_exact(&mut bytes).await.ok()?;
        match crate::from_bytes::<HostMessage>(&bytes) {
            Ok(message) => return Some((message, stream)),
            // A message whole, by its length, that this end does not know -- a newer host's:
            // skipped, and the next one read. Only a length that cannot be trusted ends it.
            Err(e) => log::warn!("a control message this session does not know: {e}"),
        }
    }
}

/// Write one control message the way [`hear`] reads it. The host's half.
pub async fn tell(stream: &mut quinn::SendStream, message: &HostMessage) -> Result<(), String> {
    let bytes = crate::to_bytes(message).map_err(|e| format!("could not encode: {e}"))?;
    stream
        .write_all(&(bytes.len() as u32).to_le_bytes())
        .await
        .map_err(|e| e.to_string())?;
    stream.write_all(&bytes).await.map_err(|e| e.to_string())
}

/// Turn what somebody typed into somewhere to connect: a name or address, with the default
/// port added when none was given.
pub async fn resolve(address: &str) -> Result<SocketAddr, String> {
    let with_port = if has_port(address) {
        address.to_string()
    } else if address.contains(':') && !address.starts_with('[') {
        // A bare IPv6 address: it has colons of its own, so the port needs brackets round it.
        format!("[{address}]:{DEFAULT_PORT}")
    } else {
        format!("{address}:{DEFAULT_PORT}")
    };
    let mut found = tokio::net::lookup_host(with_port.as_str())
        .await
        .map_err(|e| format!("cannot find {address}: {e}"))?;
    found.next().ok_or_else(|| format!("cannot find {address}"))
}

fn has_port(address: &str) -> bool {
    if let Some(rest) = address.strip_prefix('[') {
        // [v6]:port
        return rest.split_once("]:").is_some();
    }
    // name:port or v4:port — exactly one colon. More than one is a bare IPv6 address.
    address.matches(':').count() == 1
}

/// How a pairing is going.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pairing {
    /// Connected, and this is the certificate the host presented: the code is being proved
    /// against it.
    Proving { fingerprint: Fingerprint },
}

/// The outcome of a pairing both ends proved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paired {
    pub fingerprint: Fingerprint,
    /// What the host calls itself.
    pub name: String,
}

/// Pair with a host that is waiting to be paired with, using the code shown at it.
///
/// `code` is what the person typed on this device (see [`crate::pairing::typed`]); it is never
/// sent. Connects without knowing what the host is, then proves the code with the host over
/// that connection, both certificates bound in (see [`crate::pairing`]). `Ok` only when the
/// host has proved it knows the code too **and** accepted this device's proof: then, and only
/// then, may the host be written down. A wrong code, a machine in the middle, or a host that
/// lets this device in without proving anything (one built before this scheme) is an `Err`,
/// and nothing is to be written down.
pub async fn pair(
    identity: &Identity,
    address: &str,
    code: &str,
    mut progress: impl FnMut(Pairing),
) -> Result<Paired, String> {
    let code = crate::pairing::typed(code)
        .ok_or_else(|| format!("the code is the {} digits shown on the host", crate::pairing::DIGITS))?;
    let target = resolve(address).await?;
    let endpoint = client_first_contact(identity)?;
    let connection = endpoint
        .connect(target, "spatiand")
        .map_err(|e| format!("cannot reach {address}: {e}"))?;
    let connection = tokio::time::timeout(Duration::from_secs(10), connection)
        .await
        .map_err(|_| format!("{address} did not answer; is spatiand-host running on it?"))?
        .map_err(|e| format!("{address} would not connect: {e}"))?;

    let fingerprint = peer_fingerprint(&connection)
        .ok_or_else(|| "the host presented no certificate".to_string())?;
    progress(Pairing::Proving { fingerprint });

    let (prover, start) = crate::pairing::Prover::start(&code, identity.fingerprint(), fingerprint);
    say(&connection, &start).await.map_err(|_| not_pairing(address))?;

    // A host answers at once: there is nobody at it to wait for.
    let exchange = async {
        let mut control = connection.accept_uni().await.map_err(|_| not_pairing(address))?;
        let mut prover = Some(prover);
        loop {
            let Some((message, rest)) = hear(control).await else {
                return Err(if prover.is_some() {
                    not_pairing(address)
                } else {
                    format!("{address} closed the connection without letting this device in")
                });
            };
            control = rest;
            match message {
                HostMessage::PairReply { spake, confirm } => {
                    let Some(proving) = prover.take() else {
                        return Err(format!("{address} answered twice; not pairing with it"));
                    };
                    // A wrong code ends it here, before this device has said anything that
                    // would let the host check a guess: it hangs up and writes nothing down.
                    let confirmation = proving.finish(&spake, &confirm)?;
                    say(&connection, &confirmation)
                        .await
                        .map_err(|_| format!("{address} went away before the pairing was done"))?;
                }
                HostMessage::Welcome { host, .. } => {
                    if prover.is_some() {
                        // Let in without proving anything: a host from before codes were typed.
                        return Err(format!(
                            "{address} runs an older spatiand-host, which pairs without a code; \
                             update it, then pair again. Nothing was written down."
                        ));
                    }
                    return Ok(Paired { fingerprint, name: host });
                }
                HostMessage::Refused { reason, .. } => return Err(reason),
                _ => {}
            }
        }
    };
    let result = tokio::time::timeout(Duration::from_secs(30), exchange)
        .await
        .unwrap_or_else(|_| Err(format!("{address} stopped answering part way through pairing")));
    connection.close(0u32.into(), b"pairing done");
    result
}

fn not_pairing(address: &str) -> String {
    format!(
        "{address} is not expecting a new device. On it, run  spatiand-host --pair  and type the \
         code it shows within two minutes."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pairing::{Verdict, Window, WRONG_CODE};
    use crate::transport::{server, Gate, Trust};
    use std::sync::Arc;

    #[test]
    fn the_largest_paste_is_a_control_message_either_way() {
        let data = || crate::control::Clipboard::Data {
            // Longer than any real form's name.
            mime_type: "x".repeat(4096),
            bytes: vec![0xa5; crate::clipboard::LARGEST],
        };
        let up = crate::to_bytes(&ClientMessage::Clipboard(data())).unwrap();
        let down = crate::to_bytes(&HostMessage::Clipboard(data())).unwrap();
        assert!(up.len() > crate::clipboard::LARGEST && up.len() <= MAX_CONTROL);
        assert!(down.len() > crate::clipboard::LARGEST && down.len() <= MAX_CONTROL);
    }

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime")
    }

    #[test]
    fn a_typed_address_gets_the_default_port_only_when_it_has_none() {
        assert!(!has_port("workshop"));
        assert!(has_port("workshop:47600"));
        assert!(has_port("192.168.1.20:1"));
        assert!(!has_port("fe80::1"), "a bare v6 address is all colons and no port");
        assert!(has_port("[fe80::1]:47600"));
        assert!(!has_port("[fe80::1]"));
        runtime().block_on(async {
            assert_eq!(resolve("127.0.0.1").await.unwrap().port(), DEFAULT_PORT);
            assert_eq!(resolve("127.0.0.1:9").await.unwrap().port(), 9);
            assert_eq!(resolve("::1").await.unwrap().port(), DEFAULT_PORT);
        });
    }

    /// A host that lets anyone through its gate and pairs the way the real ones do: it reads
    /// what the device says, answers with [`crate::pairing::Window`], and lets the device in
    /// only on a confirmation that checks out. Each connection is one attempt; every verdict is
    /// reported on `verdicts`.
    async fn pairing_host(
        code: &'static str,
    ) -> (SocketAddr, Fingerprint, Arc<Gate>, tokio::sync::mpsc::UnboundedReceiver<Verdict>) {
        let identity = Identity::new().unwrap();
        let fingerprint = identity.fingerprint();
        let gate = Arc::new(Gate::new(Vec::new()));
        gate.set_open(true);
        let endpoint =
            server("127.0.0.1:0".parse().unwrap(), &identity, Trust::Gate(gate.clone())).unwrap();
        let address = endpoint.local_addr().unwrap();
        let (verdicts, heard) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            let mut window = Window::with_code(code.into(), Duration::from_secs(120));
            while let Some(incoming) = endpoint.accept().await {
                let Ok(connection) = incoming.await else { continue };
                let who = peer_fingerprint(&connection).unwrap();
                let Ok(mut control) = connection.open_uni().await else { continue };
                let mut verdict = None;
                while let Ok(stream) = connection.accept_uni().await {
                    let mut stream = stream;
                    let Ok(bytes) = stream.read_to_end(64 * 1024).await else { break };
                    match crate::from_bytes::<ClientMessage>(&bytes) {
                        Ok(ClientMessage::PairStart { spake }) => {
                            match window.start(fingerprint, who, &spake) {
                                Ok(reply) => {
                                    let _ = tell(&mut control, &reply).await;
                                }
                                Err(reason) => {
                                    let _ = tell(&mut control, &HostMessage::Refused { version: crate::VERSION, reason }).await;
                                    break;
                                }
                            }
                        }
                        Ok(ClientMessage::PairConfirm { confirm }) => {
                            let v = window.confirm(who, &confirm);
                            let message = match v {
                                Verdict::Paired(_) => HostMessage::Welcome {
                                    version: crate::VERSION,
                                    host: "workshop".into(),
                                    reattached: false,
                                },
                                _ => HostMessage::Refused {
                                    version: crate::VERSION,
                                    reason: WRONG_CODE.into(),
                                },
                            };
                            let _ = tell(&mut control, &message).await;
                            verdict = Some(v);
                            break;
                        }
                        _ => {}
                    }
                }
                let v = verdict.or_else(|| window.abandon(&who));
                if let Some(v) = v {
                    let _ = verdicts.send(v);
                }
                let _ = connection.closed().await;
            }
        });
        (address, fingerprint, gate, heard)
    }

    #[test]
    fn the_right_code_pairs_and_learns_the_hosts_name() {
        let session = Identity::new().unwrap();
        runtime().block_on(async {
            let (address, host, _gate, mut verdicts) = pairing_host("482913").await;
            let mut seen = Vec::new();
            let paired = pair(&session, &address.to_string(), "482 913", |p| seen.push(p))
                .await
                .expect("pairs");
            assert_eq!(paired.fingerprint, host);
            assert_eq!(paired.name, "workshop");
            assert_eq!(seen, vec![Pairing::Proving { fingerprint: host }]);
            assert_eq!(verdicts.recv().await, Some(Verdict::Paired(session.fingerprint())));
        });
    }

    #[test]
    fn a_wrong_code_is_refused_and_the_host_admits_nothing() {
        let session = Identity::new().unwrap();
        runtime().block_on(async {
            let (address, _, _gate, mut verdicts) = pairing_host("482913").await;
            let error = pair(&session, &address.to_string(), "111111", |_| {}).await.unwrap_err();
            assert!(error.contains(WRONG_CODE), "{error}");
            assert_eq!(verdicts.recv().await, Some(Verdict::Wrong { left: 2 }));
        });
    }

    #[test]
    fn three_wrong_codes_close_the_window_even_to_the_right_one() {
        let session = Identity::new().unwrap();
        runtime().block_on(async {
            let (address, _, _gate, mut verdicts) = pairing_host("482913").await;
            for typed in ["111111", "222222", "333333"] {
                assert!(pair(&session, &address.to_string(), typed, |_| {}).await.is_err());
            }
            assert_eq!(verdicts.recv().await, Some(Verdict::Wrong { left: 2 }));
            assert_eq!(verdicts.recv().await, Some(Verdict::Wrong { left: 1 }));
            assert_eq!(verdicts.recv().await, Some(Verdict::Locked));
            let error = pair(&session, &address.to_string(), "482913", |_| {}).await.unwrap_err();
            assert!(error.contains("too many"), "{error}");
        });
    }

    #[test]
    fn something_not_a_code_is_not_sent_at_all() {
        let session = Identity::new().unwrap();
        runtime().block_on(async {
            let (address, _, _gate, _verdicts) = pairing_host("482913").await;
            let error = pair(&session, &address.to_string(), "48291", |_| {}).await.unwrap_err();
            assert!(error.contains("digits"), "{error}");
        });
    }

    /// A host from before codes were typed: it lets the device in on its own say-so.
    async fn old_host() -> SocketAddr {
        let identity = Identity::new().unwrap();
        let gate = Arc::new(Gate::new(Vec::new()));
        gate.set_open(true);
        let endpoint =
            server("127.0.0.1:0".parse().unwrap(), &identity, Trust::Gate(gate.clone())).unwrap();
        let address = endpoint.local_addr().unwrap();
        tokio::spawn(async move {
            let Some(incoming) = endpoint.accept().await else { return };
            let Ok(connection) = incoming.await else { return };
            let Ok(mut control) = connection.open_uni().await else { return };
            let welcome = HostMessage::Welcome { version: crate::VERSION, host: "old".into(), reattached: false };
            let _ = tell(&mut control, &welcome).await;
            let _ = connection.closed().await;
        });
        address
    }

    #[test]
    fn a_host_that_lets_the_device_in_without_proof_is_not_paired_with() {
        let session = Identity::new().unwrap();
        runtime().block_on(async {
            let address = old_host().await;
            let error = pair(&session, &address.to_string(), "482913", |_| {}).await.unwrap_err();
            assert!(error.contains("older spatiand-host"), "{error}");
        });
    }

    #[test]
    fn a_host_that_is_not_pairing_says_how_to_make_it() {
        let session = Identity::new().unwrap();
        runtime().block_on(async {
            let (address, _, gate, _verdicts) = pairing_host("482913").await;
            gate.set_open(false);
            let error = pair(&session, &address.to_string(), "482913", |_| {}).await.unwrap_err();
            assert!(error.contains("spatiand-host --pair"), "{error}");
        });
    }
}
