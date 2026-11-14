//! An RSA key as adb knows it: what the phone is told it is when pairing, and what TLS presents
//! to the phone's adbd on every connection after.

use base64::Engine;
use num_bigint::BigUint;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

/// adb's keys are RSA, 2048 bits: the only kind adbd takes.
const MODULUS_BYTES: usize = 256;

pub struct AdbKey {
    pkcs8: Vec<u8>,
    /// The modulus, big-endian.
    modulus: Vec<u8>,
    exponent: u32,
}

impl AdbKey {
    /// A 2048-bit RSA private key in PKCS#8 DER: what Java's `KeyPairGenerator` encodes, and
    /// what adb keeps (base64, as PEM) in `~/.android/adbkey`.
    pub fn from_pkcs8(der: &[u8]) -> Result<AdbKey, String> {
        let pair = ring::rsa::KeyPair::from_pkcs8(der).map_err(|e| format!("not an RSA key adb can use: {e}"))?;
        let parts = ring::rsa::PublicKeyComponents::<Vec<u8>>::from(pair.public());
        let modulus: Vec<u8> = parts.n.iter().copied().skip_while(|b| *b == 0).collect();
        if modulus.len() != MODULUS_BYTES {
            return Err(format!("adb's keys are 2048 bits; this one is {}", modulus.len() * 8));
        }
        let exponent = parts.e.iter().fold(0u64, |e, b| e << 8 | *b as u64);
        let exponent = u32::try_from(exponent).map_err(|_| "the key's exponent is too big for adb".to_string())?;
        Ok(AdbKey { pkcs8: der.to_vec(), modulus, exponent })
    }

    /// The same, from PEM ("-----BEGIN PRIVATE KEY-----").
    pub fn from_pem(text: &str) -> Result<AdbKey, String> {
        let body: String = text.lines().filter(|l| !l.starts_with("-----")).collect();
        let der = base64::engine::general_purpose::STANDARD
            .decode(body.trim())
            .map_err(|e| format!("not PEM: {e}"))?;
        AdbKey::from_pkcs8(&der)
    }

    /// The public key in adb's own words: its structure for the key (Android's
    /// `RSAPublicKey`: the modulus and Montgomery's R² and -1/n[0] for it, little-endian),
    /// base64, then the name the phone lists the computer under -- `adbkey.pub`'s line.
    pub fn public_key(&self, name: &str) -> String {
        let n = BigUint::from_bytes_be(&self.modulus);
        let r = BigUint::from(1u8) << (MODULUS_BYTES * 8);
        let rr = (&r * &r) % &n;
        // -1/n mod 2^32: Newton's way to the inverse doubles the bits right each time.
        let n0 = u32::from_le_bytes(self.modulus[MODULUS_BYTES - 4..].iter().rev().copied().collect::<Vec<u8>>().try_into().unwrap());
        let mut inverse = 1u32;
        for _ in 0..5 {
            inverse = inverse.wrapping_mul(2u32.wrapping_sub(n0.wrapping_mul(inverse)));
        }
        let n0inv = inverse.wrapping_neg();

        let mut blob = Vec::with_capacity(4 + 4 + 2 * MODULUS_BYTES + 4);
        blob.extend_from_slice(&((MODULUS_BYTES / 4) as u32).to_le_bytes());
        blob.extend_from_slice(&n0inv.to_le_bytes());
        blob.extend(little_endian(&n, MODULUS_BYTES));
        blob.extend(little_endian(&rr, MODULUS_BYTES));
        blob.extend_from_slice(&self.exponent.to_le_bytes());
        format!("{} {name}", base64::engine::general_purpose::STANDARD.encode(blob))
    }

    /// A self-signed certificate for this key, for TLS to present. adbd looks only at the key
    /// in it: whether that is one it has paired with.
    pub(crate) fn certificate(&self) -> Result<(CertificateDer<'static>, PrivateKeyDer<'static>), String> {
        let private = PrivatePkcs8KeyDer::from(self.pkcs8.clone());
        let pair = rcgen::KeyPair::from_pkcs8_der_and_sign_algo(&private, &rcgen::PKCS_RSA_SHA256)
            .map_err(|e| format!("the key cannot sign a certificate: {e}"))?;
        let mut params = rcgen::CertificateParams::new(Vec::<String>::new()).map_err(|e| e.to_string())?;
        params.distinguished_name.push(rcgen::DnType::CommonName, "Spatiand");
        let cert = params.self_signed(&pair).map_err(|e| format!("no certificate: {e}"))?;
        Ok((cert.der().clone(), PrivateKeyDer::Pkcs8(private)))
    }
}

fn little_endian(n: &BigUint, len: usize) -> Vec<u8> {
    let mut bytes = n.to_bytes_le();
    bytes.resize(len, 0);
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A throwaway 2048-bit key made for this test with `openssl genpkey`; it opens nothing.
    const TEST_KEY: &str = include_str!("../tests/data/test-adbkey.pem");

    /// Known answer: the `adbkey.pub` line for the test key, as this code made it on
    /// num-bigint 0.4 and base64 0.22. It is what a phone stores when it pairs; another
    /// string for the same key and the phone no longer knows this computer.
    #[test]
    fn the_public_key_line_is_byte_for_byte_what_it_was() {
        let key = AdbKey::from_pem(TEST_KEY).unwrap();
        assert_eq!(key.exponent, 65537);
        let line = key.public_key("spatiand@test");
        assert_eq!(
            line,
            "QAAAADFWe90vvQlATCkbHAaeA13jFQUKg7zXFI3ifqu/RfBcOWCa3JWZFKZLmRjD0832nwCeP3WnuXPh6RPLUQlU3+JL8GwonctY6pvqpasJbdciu73/4Hjflv0koyHQOdHMiTzqys+Q3wuLm3qeCvp7yZXYmcyYPuAi7GOU44ylX4bnWgqOJWjMajTTpBqSlCAA+0gQjVWp7tDl1BhglCQad3RYVlBjrp52VFrSsCcTPpJFjPbkEEODCpI/ZhKwowtXdxjjcDq2XSO5YPEPcEzZG/KPzn8DeVHnz/39EWNakOtVOvnn7hEyqpbh65RYW/eTKujnYg5e+5wnbnum6X1rP4i8waXu76bVt/D96gJvx6xQi0pnzKpU5txBcL9ZO/ZGpk6a3qqGg4RG+A5mim/8lySArMQkG2kWL32xbSOZDaMzzIKo7ClSu9ZX88FOvOdFWh5xCG0Gu/zTl+bEDmPr6kuZXSuvCBTUw2B/hEgmG1yrlaXPb8RIj/IflgDuPRX8WEN8A9AErTQU71YkIrbhl1M1yOGzts/PWA8HYQqd9eeo0kgQAEAcLZW6ADZsaxiVrk3STclm1m/OtMD4TsndojsqT0VPYXROZRIeusuZGnP67A3A0lQMHT8I328ihSSGdHCeVrJrvMj4f47ZMCIVoAs8uxgHTP2/OgScZdbBcZCYjL2DawEAAQA= spatiand@test"
        );
        // The PEM read the same way from its DER.
        let body: String = TEST_KEY.lines().filter(|l| !l.starts_with("-----")).collect();
        assert_eq!(base64::engine::general_purpose::STANDARD.decode(&body).unwrap(), key.pkcs8);
        assert!(AdbKey::from_pem("-----BEGIN PRIVATE KEY-----\nnot base64!\n-----END PRIVATE KEY-----").is_err());
    }

    /// adb's own key pair on this computer, if it has one: the public key worked out here must
    /// be the one adb wrote next to it.
    #[test]
    #[ignore]
    fn matches_adbs_own() {
        let home = std::env::var("HOME").unwrap();
        let private = std::fs::read_to_string(format!("{home}/.android/adbkey")).expect("no ~/.android/adbkey");
        let public = std::fs::read_to_string(format!("{home}/.android/adbkey.pub")).unwrap();
        let key = AdbKey::from_pem(&private).unwrap();
        let theirs = public.trim_end_matches(['\0', '\n']).split(' ').next().unwrap().to_string();
        let mine = key.public_key("x").split(' ').next().unwrap().to_string();
        assert_eq!(mine, theirs);
    }
}
