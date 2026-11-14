//! SPAKE2 as BoringSSL has it (`crypto/curve25519/spake25519.cc`), which is what adb's pairing
//! runs: the same two points, the same password scalar -- multiples of the group's order added
//! until it is a multiple of eight -- and the same transcript hashed into the key.
//!
//! Any difference anywhere and the phone's adbd derives another key, and pairing fails with no
//! word as to why. So every step follows BoringSSL's, and the test at the bottom has been run
//! against BoringSSL itself (both roles).

use curve25519_dalek::constants::ED25519_BASEPOINT_POINT;
use curve25519_dalek::edwards::{CompressedEdwardsY, EdwardsPoint};
use curve25519_dalek::scalar::Scalar;
use curve25519_dalek::traits::Identity;
use sha2::{Digest, Sha512};

/// BoringSSL's M, the mask of the first side ("alice"): the first point on the curve the seed
/// "edwards25519 point generation seed (M)" hashes to, not multiplied by the cofactor.
const M: [u8; 32] = [
    0x5a, 0xda, 0x7e, 0x4b, 0xf6, 0xdd, 0xd9, 0xad, 0xb6, 0x62, 0x6d, 0x32, 0x13, 0x1c, 0x6b, 0x5c, 0x51, 0xa1, 0xe3,
    0x47, 0xa3, 0x47, 0x8f, 0x53, 0xcf, 0xcf, 0x44, 0x1b, 0x88, 0xee, 0xd1, 0x2e,
];
/// And N, the second side's ("bob"), from "edwards25519 point generation seed (N)".
const N: [u8; 32] = [
    0x10, 0xe3, 0xdf, 0x0a, 0xe3, 0x7d, 0x8e, 0x7a, 0x99, 0xb5, 0xfe, 0x74, 0xb4, 0x46, 0x72, 0x10, 0x3d, 0xbd, 0xdc,
    0xbd, 0x06, 0xaf, 0x68, 0x0d, 0x71, 0x32, 0x9a, 0x11, 0x69, 0x3b, 0xc7, 0x78,
];
/// The order of edwards25519's prime subgroup, l, little-endian.
const ORDER: [u8; 32] = [
    0xed, 0xd3, 0xf5, 0x5c, 0x1a, 0x63, 0x12, 0x58, 0xd6, 0x9c, 0xf7, 0xa2, 0xde, 0xf9, 0xde, 0x14, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0x10,
];

/// Which side: adb's client is alice, adbd bob.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Alice,
    Bob,
}

/// One side's half of the exchange, from its message to the shared key.
pub struct Spake2 {
    role: Role,
    my_name: Vec<u8>,
    their_name: Vec<u8>,
    private_key: [u8; 32],
    password_scalar: [u8; 32],
    password_hash: [u8; 64],
    my_msg: [u8; 32],
}

impl Spake2 {
    /// Start one side. `random` is 64 random bytes, from which the private key is made as
    /// BoringSSL makes it; names are compared byte for byte, NUL and all where adb has one.
    pub fn new(role: Role, my_name: &[u8], their_name: &[u8], password: &[u8], random: &[u8; 64]) -> Spake2 {
        // The private key: the random number modulo l, times the cofactor, so that whatever
        // small-order part the other side's point has drops out of the shared point.
        let private_key = times_eight(Scalar::from_bytes_mod_order_wide(random).to_bytes());
        let public = ED25519_BASEPOINT_POINT * Scalar::from_bytes_mod_order(private_key);

        let password_hash: [u8; 64] = Sha512::digest(password).into();
        let mut password_scalar = Scalar::from_bytes_mod_order_wide(&password_hash).to_bytes();
        // BoringSSL's correction for a cofactor it once forgot: l, 2l and 4l added as the low
        // bits say, until the scalar is a multiple of eight. The mask below is the scalar times
        // M or N *as an integer*, so these additions change the point, and must be made.
        let mut order = ORDER;
        for bit in 0..3 {
            if password_scalar[0] >> bit & 1 == 1 {
                password_scalar = add(password_scalar, order);
            }
            order = add(order, order);
        }
        debug_assert_eq!(password_scalar[0] & 7, 0);

        let mask = times(&point(if role == Role::Alice { &M } else { &N }), &password_scalar);
        let my_msg = (public + mask).compress().to_bytes();
        Spake2 {
            role,
            my_name: my_name.to_vec(),
            their_name: their_name.to_vec(),
            private_key,
            password_scalar,
            password_hash,
            my_msg,
        }
    }

    /// What to send the other side.
    pub fn message(&self) -> [u8; 32] {
        self.my_msg
    }

    /// The shared key, from the other side's message; `None` if it is not a point on the curve.
    pub fn finish(&self, their_msg: &[u8]) -> Option<[u8; 64]> {
        let their_msg: [u8; 32] = their_msg.try_into().ok()?;
        let theirs = CompressedEdwardsY(their_msg).decompress()?;
        let their_mask = times(&point(if self.role == Role::Alice { &N } else { &M }), &self.password_scalar);
        let shared = times(&(theirs - their_mask), &self.private_key).compress().to_bytes();

        let mut hash = Sha512::new();
        let mut put = |data: &[u8]| {
            hash.update((data.len() as u64).to_le_bytes());
            hash.update(data);
        };
        match self.role {
            Role::Alice => {
                put(&self.my_name);
                put(&self.their_name);
                put(&self.my_msg);
                put(&their_msg);
            }
            Role::Bob => {
                put(&self.their_name);
                put(&self.my_name);
                put(&their_msg);
                put(&self.my_msg);
            }
        }
        put(&shared);
        put(&self.password_hash);
        Some(hash.finalize().into())
    }
}

fn point(encoded: &[u8; 32]) -> EdwardsPoint {
    CompressedEdwardsY(*encoded).decompress().expect("M and N are points on the curve")
}

/// `point` times a 256-bit little-endian integer, not reduced modulo l: M and N have parts
/// outside the prime subgroup, and BoringSSL multiplies by the whole integer.
fn times(point: &EdwardsPoint, scalar: &[u8; 32]) -> EdwardsPoint {
    let mut sum = EdwardsPoint::identity();
    for bit in (0..256).rev() {
        sum = sum + sum;
        if scalar[bit / 8] >> (bit % 8) & 1 == 1 {
            sum += point;
        }
    }
    sum
}

/// Two 256-bit little-endian integers added; neither of BoringSSL's sums overflows.
fn add(a: [u8; 32], b: [u8; 32]) -> [u8; 32] {
    let mut out = [0u8; 32];
    let mut carry = 0u16;
    for i in 0..32 {
        let sum = a[i] as u16 + b[i] as u16 + carry;
        out[i] = sum as u8;
        carry = sum >> 8;
    }
    out
}

/// BoringSSL's `left_shift_3`.
fn times_eight(n: [u8; 32]) -> [u8; 32] {
    let mut out = [0u8; 32];
    let mut carry = 0u8;
    for i in 0..32 {
        out[i] = n[i] << 3 | carry;
        carry = n[i] >> 5;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLIENT: &[u8] = b"adb pair client\0";
    const SERVER: &[u8] = b"adb pair server\0";

    fn random(seed: u8) -> [u8; 64] {
        let mut r = [0u8; 64];
        for (i, b) in r.iter_mut().enumerate() {
            *b = (i as u8).wrapping_mul(31).wrapping_add(seed);
        }
        r
    }

    #[test]
    fn both_sides_agree() {
        let alice = Spake2::new(Role::Alice, CLIENT, SERVER, b"123456", &random(1));
        let bob = Spake2::new(Role::Bob, SERVER, CLIENT, b"123456", &random(2));
        let a = alice.finish(&bob.message()).unwrap();
        let b = bob.finish(&alice.message()).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn a_wrong_password_gives_another_key() {
        let alice = Spake2::new(Role::Alice, CLIENT, SERVER, b"123456", &random(1));
        let bob = Spake2::new(Role::Bob, SERVER, CLIENT, b"654321", &random(2));
        assert_ne!(alice.finish(&bob.message()).unwrap(), bob.finish(&alice.message()).unwrap());
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Known answers: both sides' messages and the key, byte for byte, as this code gave them
    /// on curve25519-dalek 4 and sha2 0.10 (which agree with BoringSSL, `agrees_with_boringssl`).
    /// A library upgrade that changes any of them would leave every phone unable to pair.
    #[test]
    fn the_exchange_is_byte_for_byte_what_it_was() {
        let alice = Spake2::new(Role::Alice, CLIENT, SERVER, b"123456", &random(1));
        let bob = Spake2::new(Role::Bob, SERVER, CLIENT, b"123456", &random(2));
        assert_eq!(hex(&alice.message()), "c5fb6d957be16ad99a89d359817ee822ef601d1717911d47352af1fa7ae580a5");
        assert_eq!(hex(&bob.message()), "253f477cf198ec897cb2b163b67f0178415d95441431dbeaa32bf01d3e0c6ba7");
        assert_eq!(hex(&alice.finish(&bob.message()).unwrap()), "fb50aba112cbb08159266c9baa5aeb8dc5e7247cc9d736c09ea4e92f137d6fb066f2d1fb7d9fc979cd0d122b62d56a44e048c2341d8064dbbf6680ffc430ffb4");
        assert_eq!(hex(&alice.password_scalar), "0848c149c51f44ac0257d23a85c552d9f21ad341c5cfc475aee338f3cfc0956c");
        // What a phone could send that is no message: the wrong length, or no point at all
        // (y = 2 is not on the curve).
        assert!(alice.finish(&[1u8; 31]).is_none());
        let mut not_a_point = [0u8; 32];
        not_a_point[0] = 2;
        assert!(alice.finish(&not_a_point).is_none());
    }

    #[test]
    fn the_password_scalar_is_a_multiple_of_eight() {
        for seed in 0..32u8 {
            let s = Spake2::new(Role::Alice, CLIENT, SERVER, &[seed, 1, 2], &random(seed));
            assert_eq!(s.password_scalar[0] & 7, 0);
        }
    }

    /// Against BoringSSL itself: `spake_peer` (BoringSSL's SPAKE2 with adb's names, built from
    /// Chromium's copy of it) run over ssh, in both roles. `SPAKE_PEER="ssh host /path/spake_peer"`.
    #[test]
    #[ignore]
    fn agrees_with_boringssl() {
        use std::io::{BufRead, BufReader, Write};
        let peer = std::env::var("SPAKE_PEER").expect("SPAKE_PEER");
        let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
        let unhex = |s: &str| (0..s.len() / 2).map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap()).collect::<Vec<u8>>();
        for (mine, theirs) in [(Role::Alice, "bob"), (Role::Bob, "alice")] {
            let password = b"482915\x01\x02\x03 and the exporter's 64 bytes, or anything";
            let (my_name, their_name) = if mine == Role::Alice { (CLIENT, SERVER) } else { (SERVER, CLIENT) };
            let me = Spake2::new(mine, my_name, their_name, password, &random(7));
            let mut child = std::process::Command::new("sh")
                .arg("-c")
                .arg(format!("{peer} {theirs} {}", hex(password)))
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            let mut out = BufReader::new(child.stdout.take().unwrap());
            let mut line = String::new();
            out.read_line(&mut line).unwrap();
            let their_msg = unhex(line.trim());
            writeln!(child.stdin.take().unwrap(), "{}", hex(&me.message())).unwrap();
            line.clear();
            out.read_line(&mut line).unwrap();
            let their_key = unhex(line.trim());
            assert_eq!(me.finish(&their_msg).unwrap().to_vec(), their_key, "as {mine:?}");
            child.wait().unwrap();
        }
    }
}
