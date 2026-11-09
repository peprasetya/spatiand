//! Pairing with a phone's Wireless debugging: what `adb pair HOST:PORT CODE` does, after which
//! the phone's adbd takes this key on its connection port (see `connection`).
//!
//! adb's `pairing_connection`, step by step: TLS 1.3; both sides export 64 bytes of keying
//! material ("adb-label", NUL included) and add them to the six digits Settings shows to make
//! SPAKE2's password; the SPAKE2 messages go each way; the key it gives is made an AES-128-GCM
//! key by HKDF; and each side sends the other a sealed `PeerInfo` -- this side its public key
//! and name, the phone its GUID. A wrong code gives the phone another key, so it cannot open
//! what was sent, and closes the connection.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_128_GCM};

use crate::key::AdbKey;
use crate::spake2::{Role, Spake2};

const CLIENT_NAME: &[u8] = b"adb pair client\0";
const SERVER_NAME: &[u8] = b"adb pair server\0";
const EXPORTED_LABEL: &[u8] = b"adb-label\0";
const HKDF_INFO: &[u8] = b"adb pairing_auth aes-128-gcm key";
/// A `PeerInfo`'s size: its type, then its data, zero-padded.
const PEER_INFO_SIZE: usize = 1 << 13;
const HEADER_VERSION: u8 = 1;
const SPAKE2_MSG: u8 = 0;
const PEER_INFO: u8 = 1;
const RSA_PUBLIC_KEY: u8 = 0;
const DEVICE_GUID: u8 = 1;

/// Pair `key`, listed on the phone as `name`, with the pairing service at `address` using the
/// code Settings shows. Returns the phone's GUID.
pub fn pair(address: SocketAddr, code: &str, key: &AdbKey, name: &str) -> Result<String, String> {
    let socket = TcpStream::connect_timeout(&address, Duration::from_secs(5))
        .map_err(|e| format!("could not reach the pairing service at {address}: {e}"))?;
    socket.set_read_timeout(Some(Duration::from_secs(20))).ok();
    socket.set_write_timeout(Some(Duration::from_secs(20))).ok();
    socket.set_nodelay(true).ok();
    let mut tls = crate::tls::connect(key, address.ip(), socket)?;

    let mut exported = [0u8; 64];
    tls.conn
        .export_keying_material(&mut exported, EXPORTED_LABEL, None)
        .map_err(|e| format!("no keying material: {e}"))?;
    let mut password = code.trim().as_bytes().to_vec();
    password.extend_from_slice(&exported);
    let mut random = [0u8; 64];
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut random).map_err(|_| "no randomness")?;
    let spake = Spake2::new(Role::Alice, CLIENT_NAME, SERVER_NAME, &password, &random);

    send(&mut tls, SPAKE2_MSG, &spake.message())?;
    let theirs = receive(&mut tls, SPAKE2_MSG)?;
    let shared = spake.finish(&theirs).ok_or("the phone's half of the exchange is not a point on the curve")?;
    let mut cipher = Cipher::new(&shared)?;

    let mut info = vec![0u8; PEER_INFO_SIZE];
    info[0] = RSA_PUBLIC_KEY;
    let public = key.public_key(name);
    if public.len() >= PEER_INFO_SIZE - 1 {
        return Err("the public key is too long to send".into());
    }
    info[1..1 + public.len()].copy_from_slice(public.as_bytes());
    send(&mut tls, PEER_INFO, &cipher.seal(info)?)
        .map_err(|e| format!("{e} (was the code right?)"))?;
    let sealed = receive(&mut tls, PEER_INFO).map_err(|e| format!("the phone did not take the code: {e}"))?;
    let theirs = cipher.open(sealed).ok_or("the phone's answer could not be opened: the code was not the one it showed")?;
    if theirs.first() != Some(&DEVICE_GUID) {
        return Err("the phone answered with something other than its GUID".into());
    }
    let guid: Vec<u8> = theirs[1..].iter().copied().take_while(|b| *b != 0).collect();
    Ok(String::from_utf8_lossy(&guid).into_owned())
}

/// A packet: its header (version, type, payload's length big-endian), then the payload.
fn send(tls: &mut impl Write, kind: u8, payload: &[u8]) -> Result<(), String> {
    let mut packet = vec![HEADER_VERSION, kind];
    packet.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    packet.extend_from_slice(payload);
    tls.write_all(&packet).and_then(|_| tls.flush()).map_err(|e| format!("could not send: {e}"))
}

fn receive(tls: &mut impl Read, kind: u8) -> Result<Vec<u8>, String> {
    let mut header = [0u8; 6];
    tls.read_exact(&mut header).map_err(|e| format!("the phone closed the pairing ({e})"))?;
    let length = u32::from_be_bytes(header[2..6].try_into().unwrap()) as usize;
    if header[0] != HEADER_VERSION || header[1] != kind || length > 2 * PEER_INFO_SIZE {
        return Err(format!("not the packet expected: {header:02x?}"));
    }
    let mut payload = vec![0u8; length];
    tls.read_exact(&mut payload).map_err(|e| format!("the phone closed the pairing ({e})"))?;
    Ok(payload)
}

/// adb's `Aes128Gcm`: the key from HKDF-SHA256 over SPAKE2's (no salt), and each direction's
/// nonce a counter from 0, little-endian, in the first eight of its twelve bytes.
struct Cipher {
    key: LessSafeKey,
    sealed: u64,
    opened: u64,
}

impl Cipher {
    fn new(shared: &[u8]) -> Result<Cipher, String> {
        struct Len(usize);
        impl ring::hkdf::KeyType for Len {
            fn len(&self) -> usize {
                self.0
            }
        }
        let mut key = [0u8; 16];
        ring::hkdf::Salt::new(ring::hkdf::HKDF_SHA256, &[])
            .extract(shared)
            .expand(&[HKDF_INFO], Len(16))
            .and_then(|okm| okm.fill(&mut key))
            .map_err(|_| "HKDF failed")?;
        let key = UnboundKey::new(&AES_128_GCM, &key).map_err(|_| "no AES key")?;
        Ok(Cipher { key: LessSafeKey::new(key), sealed: 0, opened: 0 })
    }

    fn nonce(counter: u64) -> Nonce {
        let mut bytes = [0u8; 12];
        bytes[..8].copy_from_slice(&counter.to_le_bytes());
        Nonce::assume_unique_for_key(bytes)
    }

    fn seal(&mut self, mut plain: Vec<u8>) -> Result<Vec<u8>, String> {
        let nonce = Cipher::nonce(self.sealed);
        self.sealed += 1;
        self.key.seal_in_place_append_tag(nonce, Aad::empty(), &mut plain).map_err(|_| "could not seal")?;
        Ok(plain)
    }

    fn open(&mut self, mut sealed: Vec<u8>) -> Option<Vec<u8>> {
        let nonce = Cipher::nonce(self.opened);
        self.opened += 1;
        let plain = self.key.open_in_place(nonce, Aad::empty(), &mut sealed).ok()?;
        Some(plain.to_vec())
    }
}
