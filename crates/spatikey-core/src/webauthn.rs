//! The authenticator: making passkeys and signing with them (WebAuthn Level 3, ES256, "none"
//! attestation, synced-passkey flags).

use ring::rand::{SecureRandom, SystemRandom};
use ring::signature::{EcdsaKeyPair, KeyPair, ECDSA_P256_SHA256_ASN1_SIGNING};
use zeroize::Zeroizing;

use crate::b64;
use crate::cbor::{self, Value};
use crate::entry::Passkey;
use crate::wire::{AssertedAnswer, Code, CreatedAnswer, PasskeyCreate, Refusal};

/// SpatiKey's AAGUID: its own, the same on every machine (a site may show it as the
/// authenticator's name). 5b9e3f0a-c1d2-4e5f-8a6b-7c8d9e0f1a2b.
pub const AAGUID: [u8; 16] = [
    0x5b, 0x9e, 0x3f, 0x0a, 0xc1, 0xd2, 0x4e, 0x5f, 0x8a, 0x6b, 0x7c, 0x8d, 0x9e, 0x0f, 0x1a, 0x2b,
];

/// COSE's number for ES256 (ECDSA on P-256 with SHA-256), the one algorithm SpatiKey makes.
pub const ES256: i64 = -7;

// The authenticator data's flags. Every use is a finger on a Mac, so the user was both present
// (UP) and verified (UV); a passkey in the synced vault may be backed up (BE) and is (BS); and
// when one is made, its credential follows (AT).
const UP: u8 = 0x01;
const UV: u8 = 0x04;
const BE: u8 = 0x08;
const BS: u8 = 0x10;
const AT: u8 = 0x40;

/// The DER of a P-256 public key's SubjectPublicKeyInfo up to the point itself, which is the
/// same for every key: SEQUENCE { SEQUENCE { id-ecPublicKey, prime256v1 }, BIT STRING with no
/// unused bits, holding the 65-byte uncompressed point }.
const P256_SPKI_PREFIX: [u8; 26] = [
    0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08, 0x2a, 0x86, 0x48,
    0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00,
];

/// A new passkey and what the page is given.
pub struct Created {
    pub passkey: Passkey,
    pub answer: CreatedAnswer,
}

/// Make a passkey for `request`: an ES256 key (refused with `bad_request` if the site's `algs`
/// are given and do not include -7), a random 16-byte credential id, authenticator data with
/// UP, UV, BE, BS and AT set and a counter of 0, and a "none" attestation object.
///
/// Also refused with `bad_request`: an rp id the origin may not use ([`crate::site::rp_id_ok`],
/// checked here too so that no way of reaching this skips it), and a user id that is not 1 to 64
/// bytes of base64url, as WebAuthn requires. The client data hash is not used: a "none"
/// attestation signs nothing.
pub fn create(request: &PasskeyCreate) -> Result<Created, Refusal> {
    if !request.algs.is_empty() && !request.algs.contains(&ES256) {
        return Err(Refusal::new(Code::BadRequest, "SpatiKey makes ES256 passkeys only"));
    }
    if !crate::site::rp_id_ok(&request.rp_id, &request.origin) {
        return Err(Refusal::new(
            Code::BadRequest,
            format!("{} may not make a passkey for \"{}\"", request.origin, request.rp_id),
        ));
    }
    let user_handle = b64::decode_url(&request.user_id)
        .map_err(|_| Refusal::new(Code::BadRequest, "the user id is not base64url"))?;
    if user_handle.is_empty() || user_handle.len() > 64 {
        return Err(Refusal::new(Code::BadRequest, "a user id is 1 to 64 bytes"));
    }

    let rng = SystemRandom::new();
    let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &rng)
        .map_err(|_| failed("could not make a key"))?;
    let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, pkcs8.as_ref(), &rng)
        .map_err(|_| failed("could not read the key just made"))?;
    // 0x04, then x and y, 32 bytes each.
    let point = pair.public_key().as_ref();
    if point.len() != 65 || point[0] != 0x04 {
        return Err(failed("the key just made has an unexpected public key"));
    }
    let mut credential_id = [0u8; 16];
    rng.fill(&mut credential_id).map_err(|_| failed("no random numbers"))?;

    let cose_key = cbor::encode(&Value::Map(vec![
        // kty: EC2; alg: ES256; crv: P-256; x; y. CTAP2's canonical order sorts keys by their
        // encoding (shorter first, then bytewise): 1, 3, -1, -2, -3.
        (Value::Int(1), Value::Int(2)),
        (Value::Int(3), Value::Int(ES256)),
        (Value::Int(-1), Value::Int(1)),
        (Value::Int(-2), Value::Bytes(point[1..33].to_vec())),
        (Value::Int(-3), Value::Bytes(point[33..65].to_vec())),
    ]));
    let mut authenticator_data = Vec::with_capacity(37 + AAGUID.len() + 2 + credential_id.len() + cose_key.len());
    authenticator_data.extend_from_slice(&rp_id_hash(&request.rp_id));
    authenticator_data.push(UP | UV | BE | BS | AT);
    authenticator_data.extend_from_slice(&0u32.to_be_bytes());
    authenticator_data.extend_from_slice(&AAGUID);
    authenticator_data.extend_from_slice(&(credential_id.len() as u16).to_be_bytes());
    authenticator_data.extend_from_slice(&credential_id);
    authenticator_data.extend_from_slice(&cose_key);
    // Canonical order again: "fmt" (3 letters), "attStmt" (7), "authData" (8).
    let attestation_object = cbor::encode(&Value::Map(vec![
        (Value::Text("fmt".into()), Value::Text("none".into())),
        (Value::Text("attStmt".into()), Value::Map(Vec::new())),
        (Value::Text("authData".into()), Value::Bytes(authenticator_data.clone())),
    ]));
    let mut public_key = Vec::with_capacity(P256_SPKI_PREFIX.len() + point.len());
    public_key.extend_from_slice(&P256_SPKI_PREFIX);
    public_key.extend_from_slice(point);

    let passkey = Passkey {
        rp_id: request.rp_id.clone(),
        rp_name: request.rp_name.clone(),
        credential_id: b64::encode_url(&credential_id),
        // Written again rather than copied, so that it is kept without the padding a page may
        // have sent.
        user_handle: b64::encode_url(&user_handle),
        user_name: request.user_name.clone(),
        user_display: request.user_display.clone(),
        key: b64::encode(pkcs8.as_ref()),
        alg: ES256,
        sign_count: 0,
    };
    let answer = CreatedAnswer {
        credential_id: passkey.credential_id.clone(),
        authenticator_data: b64::encode_url(&authenticator_data),
        attestation_object: b64::encode_url(&attestation_object),
        public_key: b64::encode_url(&public_key),
        alg: ES256,
        transports: vec!["internal".into()],
    };
    Ok(Created { passkey, answer })
}

/// Sign in with `passkey`: authenticator data (UP, UV, BE, BS, counter 0) and an ES256
/// signature (ASN.1 DER) over it and the client data hash.
///
/// Whether the page may use the passkey's rp id is the caller's to have checked: nothing here
/// knows the origin.
pub fn assert(passkey: &Passkey, client_data_hash: &[u8]) -> Result<AssertedAnswer, Refusal> {
    if client_data_hash.len() != 32 {
        return Err(Refusal::new(Code::BadRequest, "the client data hash is not 32 bytes"));
    }
    if passkey.alg != ES256 {
        return Err(failed("SpatiKey signs with ES256 passkeys only"));
    }
    // Read and written again, so that what the page is given is always plain base64url. The
    // errors say which field is damaged and nothing of what is in it.
    let credential_id =
        b64::decode_url(&passkey.credential_id).map_err(|_| failed("the passkey's credential id is damaged"))?;
    let user_handle =
        b64::decode_url(&passkey.user_handle).map_err(|_| failed("the passkey's user handle is damaged"))?;
    let pkcs8 = Zeroizing::new(b64::decode(&passkey.key).map_err(|_| failed("the passkey's key is damaged"))?);
    let rng = SystemRandom::new();
    let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &pkcs8, &rng)
        .map_err(|_| failed("the passkey's key is damaged"))?;

    let mut authenticator_data = Vec::with_capacity(37);
    authenticator_data.extend_from_slice(&rp_id_hash(&passkey.rp_id));
    authenticator_data.push(UP | UV | BE | BS);
    // Always 0: a passkey kept on several machines cannot count its uses.
    authenticator_data.extend_from_slice(&0u32.to_be_bytes());
    let mut signed = Vec::with_capacity(authenticator_data.len() + client_data_hash.len());
    signed.extend_from_slice(&authenticator_data);
    signed.extend_from_slice(client_data_hash);
    let signature = pair.sign(&rng, &signed).map_err(|_| failed("could not sign"))?;
    Ok(AssertedAnswer {
        credential_id: b64::encode_url(&credential_id),
        authenticator_data: b64::encode_url(&authenticator_data),
        signature: b64::encode_url(signature.as_ref()),
        user_handle: b64::encode_url(&user_handle),
    })
}

/// clientDataJSON exactly as browsers write it: `type`, `challenge`, `origin`, `crossOrigin`, and
/// `topOrigin` when the frame is not the top one, in that order, with JSON string escaping.
///
/// `challenge` is the challenge's base64url; padding, if the caller left it on, is dropped, as
/// browsers write none. `topOrigin` is written whenever one is given. Strings are escaped as
/// serde_json escapes them. For everything a browser actually writes here (a fixed type, a
/// base64url challenge, a serialized origin) there is nothing to escape, so browsers, whose
/// escapers differ only in characters an origin cannot hold, all agree with it.
pub fn client_data_json(kind: &str, challenge: &str, origin: &str, cross_origin: bool, top_origin: Option<&str>) -> String {
    let mut out = String::with_capacity(160);
    out.push_str("{\"type\":");
    out.push_str(&json_string(kind));
    out.push_str(",\"challenge\":");
    out.push_str(&json_string(challenge.trim_end_matches('=')));
    out.push_str(",\"origin\":");
    out.push_str(&json_string(origin));
    out.push_str(",\"crossOrigin\":");
    out.push_str(if cross_origin { "true" } else { "false" });
    if let Some(top_origin) = top_origin {
        out.push_str(",\"topOrigin\":");
        out.push_str(&json_string(top_origin));
    }
    out.push('}');
    out
}

/// `text` as a JSON string, quotes included.
fn json_string(text: &str) -> String {
    serde_json::Value::from(text).to_string()
}

/// SHA-256 of an rp id.
pub fn rp_id_hash(rp_id: &str) -> [u8; 32] {
    let digest = ring::digest::digest(&ring::digest::SHA256, rp_id.as_bytes());
    let mut out = [0u8; 32];
    out.copy_from_slice(digest.as_ref());
    out
}

fn failed(message: &str) -> Refusal {
    Refusal::new(Code::Failed, message)
}

#[cfg(test)]
mod tests {
    use ring::signature::{UnparsedPublicKey, ECDSA_P256_SHA256_ASN1};

    use super::*;

    fn request() -> PasskeyCreate {
        PasskeyCreate {
            origin: "https://login.example.co.uk".into(),
            rp_id: "example.co.uk".into(),
            rp_name: "Example".into(),
            user_id: b64::encode_url(b"user-1"),
            user_name: "paulus@example.co.uk".into(),
            user_display: "Paulus".into(),
            client_data_hash: b64::encode_url(&[7u8; 32]),
            exclude: Vec::new(),
            algs: vec![-8, -7, -257],
        }
    }

    fn text(s: &str) -> Value {
        Value::Text(s.into())
    }

    fn sha256(bytes: &[u8]) -> Vec<u8> {
        ring::digest::digest(&ring::digest::SHA256, bytes).as_ref().to_vec()
    }

    #[test]
    fn a_new_passkeys_attestation_reads_as_webauthn_says() {
        let created = create(&request()).unwrap();
        let answer = &created.answer;
        assert_eq!(answer.alg, -7);
        assert_eq!(answer.transports, ["internal"]);

        let object = b64::decode_url(&answer.attestation_object).unwrap();
        let (object, rest) = cbor::decode(&object).unwrap();
        assert!(rest.is_empty());
        let Value::Map(pairs) = &object else { panic!("the attestation object is not a map") };
        let keys: Vec<_> = pairs.iter().map(|(key, _)| key.as_text().unwrap()).collect();
        assert_eq!(keys, ["fmt", "attStmt", "authData"]);
        assert_eq!(object.get(&text("fmt")), Some(&text("none")));
        assert_eq!(object.get(&text("attStmt")), Some(&Value::Map(Vec::new())));

        let data = object.get(&text("authData")).and_then(Value::as_bytes).unwrap();
        assert_eq!(data, b64::decode_url(&answer.authenticator_data).unwrap());
        assert_eq!(&data[..32], sha256(b"example.co.uk"));
        // UP, UV, BE, BS and AT; not ED: there are no extensions.
        assert_eq!(data[32], 0b0101_1101);
        assert_eq!(&data[33..37], [0, 0, 0, 0]);
        assert_eq!(&data[37..53], AAGUID);
        assert_eq!(&data[53..55], [0, 16]);
        let credential_id = &data[55..71];
        assert_eq!(credential_id, b64::decode_url(&answer.credential_id).unwrap());

        let (cose, rest) = cbor::decode(&data[71..]).unwrap();
        assert!(rest.is_empty(), "nothing follows the key");
        let Value::Map(pairs) = &cose else { panic!("the COSE key is not a map") };
        let keys: Vec<_> = pairs.iter().map(|(key, _)| key.as_int().unwrap()).collect();
        assert_eq!(keys, [1, 3, -1, -2, -3]);
        assert_eq!(cose.get(&Value::Int(1)), Some(&Value::Int(2)));
        assert_eq!(cose.get(&Value::Int(3)), Some(&Value::Int(-7)));
        assert_eq!(cose.get(&Value::Int(-1)), Some(&Value::Int(1)));
        let x = cose.get(&Value::Int(-2)).and_then(Value::as_bytes).unwrap();
        let y = cose.get(&Value::Int(-3)).and_then(Value::as_bytes).unwrap();
        assert_eq!((x.len(), y.len()), (32, 32));

        // getPublicKey(): the same point, in a SubjectPublicKeyInfo.
        let spki = b64::decode_url(&answer.public_key).unwrap();
        assert_eq!(spki.len(), 91);
        assert_eq!(
            &spki[..26],
            [
                0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08, 0x2a, 0x86,
                0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00
            ]
        );
        assert_eq!(spki[26], 0x04);
        assert_eq!(&spki[27..59], x);
        assert_eq!(&spki[59..91], y);

        let passkey = &created.passkey;
        assert_eq!(passkey.rp_id, "example.co.uk");
        assert_eq!(passkey.rp_name, "Example");
        assert_eq!(passkey.credential_id, answer.credential_id);
        assert_eq!(passkey.user_handle, b64::encode_url(b"user-1"));
        assert_eq!((passkey.user_name.as_str(), passkey.user_display.as_str()), ("paulus@example.co.uk", "Paulus"));
        assert_eq!((passkey.alg, passkey.sign_count), (-7, 0));
        // PKCS#8 in standard base64, and a key ring reads back.
        let pkcs8 = b64::decode(&passkey.key).unwrap();
        EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &pkcs8, &SystemRandom::new()).unwrap();
    }

    #[test]
    fn a_sign_in_verifies_with_the_key_the_site_was_given() {
        let created = create(&request()).unwrap();
        let cose = b64::decode_url(&created.answer.authenticator_data).unwrap();
        let (cose, _) = cbor::decode(&cose[71..]).unwrap();
        let mut point = vec![0x04];
        point.extend_from_slice(cose.get(&Value::Int(-2)).and_then(Value::as_bytes).unwrap());
        point.extend_from_slice(cose.get(&Value::Int(-3)).and_then(Value::as_bytes).unwrap());
        let spki = b64::decode_url(&created.answer.public_key).unwrap();

        let client_data = client_data_json("webauthn.get", "Y2hhbGxlbmdl", "https://login.example.co.uk", false, None);
        let hash = sha256(client_data.as_bytes());
        let asserted = assert(&created.passkey, &hash).unwrap();
        assert_eq!(asserted.credential_id, created.answer.credential_id);
        assert_eq!(asserted.user_handle, b64::encode_url(b"user-1"));

        let data = b64::decode_url(&asserted.authenticator_data).unwrap();
        assert_eq!(data.len(), 37);
        assert_eq!(&data[..32], sha256(b"example.co.uk"));
        assert_eq!(data[32], 0b0001_1101);
        assert_eq!(&data[33..], [0, 0, 0, 0]);

        let signature = b64::decode_url(&asserted.signature).unwrap();
        assert_eq!(signature[0], 0x30, "an ASN.1 DER SEQUENCE");
        let mut signed = data.clone();
        signed.extend_from_slice(&hash);
        UnparsedPublicKey::new(&ECDSA_P256_SHA256_ASN1, &point).verify(&signed, &signature).unwrap();
        // The SubjectPublicKeyInfo carries the same key.
        UnparsedPublicKey::new(&ECDSA_P256_SHA256_ASN1, &spki[26..]).verify(&signed, &signature).unwrap();
        // Over anything else, it does not verify.
        let mut other = signed.clone();
        other[40] ^= 1;
        assert!(UnparsedPublicKey::new(&ECDSA_P256_SHA256_ASN1, &point).verify(&other, &signature).is_err());
        // Nor does another passkey's signature.
        let stranger = assert(&create(&request()).unwrap().passkey, &hash).unwrap();
        let stranger = b64::decode_url(&stranger.signature).unwrap();
        assert!(UnparsedPublicKey::new(&ECDSA_P256_SHA256_ASN1, &point).verify(&signed, &stranger).is_err());
    }

    #[test]
    fn every_passkey_is_its_own() {
        let a = create(&request()).unwrap();
        let b = create(&request()).unwrap();
        assert_ne!(a.passkey.credential_id, b.passkey.credential_id);
        assert_ne!(a.answer.public_key, b.answer.public_key);
        assert_eq!(b64::decode_url(&a.passkey.credential_id).unwrap().len(), 16);
    }

    #[test]
    fn makes_es256_only() {
        let mut rsa_only = request();
        rsa_only.algs = vec![-257];
        let refusal = create(&rsa_only).err().unwrap();
        assert_eq!(refusal, Refusal::new(Code::BadRequest, "SpatiKey makes ES256 passkeys only"));
        let mut any = request();
        any.algs.clear();
        assert!(create(&any).is_ok());
    }

    #[test]
    fn refuses_a_user_id_webauthn_would_not_take() {
        for user_id in [String::new(), "not base64url!".into(), b64::encode_url(&[1u8; 65])] {
            let mut bad = request();
            bad.user_id = user_id;
            assert_eq!(create(&bad).err().unwrap().code, Code::BadRequest);
        }
        let mut longest = request();
        longest.user_id = b64::encode_url(&[1u8; 64]);
        assert!(create(&longest).is_ok());
        // Padded, as some pages send it: kept without.
        let mut padded = request();
        padded.user_id = format!("{}==", b64::encode_url(b"user-12"));
        assert_eq!(create(&padded).unwrap().passkey.user_handle, b64::encode_url(b"user-12"));
    }

    #[test]
    fn refuses_an_rp_id_the_origin_may_not_use() {
        for (rp_id, origin) in [
            ("example.com", "https://evil.com"),
            ("co.uk", "https://login.example.co.uk"),
            ("example.co.uk", "http://login.example.co.uk"),
            ("192.168.1.2", "https://192.168.1.2"),
        ] {
            let mut bad = request();
            bad.rp_id = rp_id.into();
            bad.origin = origin.into();
            assert_eq!(create(&bad).err().unwrap().code, Code::BadRequest, "{rp_id} for {origin}");
        }
        let mut local = request();
        local.rp_id = "localhost".into();
        local.origin = "http://localhost:8080".into();
        assert!(create(&local).is_ok());
    }

    #[test]
    fn a_sign_in_needs_a_whole_hash_and_a_sound_key() {
        let passkey = create(&request()).unwrap().passkey;
        assert_eq!(assert(&passkey, &[0u8; 31]).err().unwrap().code, Code::BadRequest);
        assert_eq!(assert(&passkey, &[0u8; 33]).err().unwrap().code, Code::BadRequest);

        let mut damaged = passkey.clone();
        damaged.key = b64::encode(b"not a key at all");
        let refusal = assert(&damaged, &[0u8; 32]).err().unwrap();
        assert_eq!(refusal.code, Code::Failed);
        let mut garbled = passkey.clone();
        garbled.key.insert(5, '!');
        let refusal = assert(&garbled, &[0u8; 32]).err().unwrap();
        assert_eq!(refusal.code, Code::Failed);
        // The message names the field, never what is in it.
        assert!(!refusal.message.contains(&passkey.key[..12]));
        assert!(!refusal.message.contains('!'));

        let mut other = passkey.clone();
        other.alg = -257;
        assert_eq!(assert(&other, &[0u8; 32]).err().unwrap().code, Code::Failed);
    }

    #[test]
    fn client_data_is_written_as_browsers_write_it() {
        assert_eq!(
            client_data_json("webauthn.create", "AAEC", "https://github.com", false, None),
            r#"{"type":"webauthn.create","challenge":"AAEC","origin":"https://github.com","crossOrigin":false}"#
        );
        assert_eq!(
            client_data_json("webauthn.get", "q83v_-A", "https://pay.example", true, Some("https://shop.example")),
            r#"{"type":"webauthn.get","challenge":"q83v_-A","origin":"https://pay.example","crossOrigin":true,"topOrigin":"https://shop.example"}"#
        );
        // Padding off the challenge, as browsers write it.
        assert_eq!(
            client_data_json("webauthn.get", "AAE=", "http://localhost:8080", false, None),
            r#"{"type":"webauthn.get","challenge":"AAE","origin":"http://localhost:8080","crossOrigin":false}"#
        );
        // What would need escaping, escaped.
        assert_eq!(
            client_data_json("webauthn.get", "AA", "https://a\"b\\c\n\u{1}\u{e9}", false, None),
            "{\"type\":\"webauthn.get\",\"challenge\":\"AA\",\"origin\":\"https://a\\\"b\\\\c\\n\\u0001\u{e9}\",\"crossOrigin\":false}"
        );
    }

    #[test]
    fn client_data_reads_back_as_json() {
        let text = client_data_json("webauthn.create", "Y2g", "https://example.com", true, Some("https://top.example"));
        let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed["type"], "webauthn.create");
        assert_eq!(parsed["challenge"], "Y2g");
        assert_eq!(parsed["origin"], "https://example.com");
        assert_eq!(parsed["crossOrigin"], true);
        assert_eq!(parsed["topOrigin"], "https://top.example");
    }
}
