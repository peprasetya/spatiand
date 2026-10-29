//! One-time codes (RFC 6238 over RFC 4226), and `otpauth://` links (Google Authenticator's
//! key-uri format).
//!
//! The secret is never put in an error: a link that does not read says which part is wrong, not
//! what was in it.

use std::sync::OnceLock;

use data_encoding::{Encoding, BASE32_NOPAD};
use ring::hmac;
use zeroize::Zeroizing;

use crate::entry::Totp;

/// Read an `otpauth://totp/Issuer:label?secret=...&issuer=...&digits=...&period=...&algorithm=...`
/// link. `hotp` is refused.
///
/// The label is percent-decoded and split at its first colon into the issuer and the account;
/// an `issuer` parameter, when there is one, is preferred to the label's. What is not given is
/// the default: six digits, thirty seconds, SHA1. Parameters other than these are ignored.
pub fn parse_uri(uri: &str) -> Result<Totp, String> {
    let uri = uri.trim();
    let rest = match uri.get(..10) {
        Some(scheme) if scheme.eq_ignore_ascii_case("otpauth://") => &uri[10..],
        _ => return Err("not an otpauth:// link".into()),
    };
    let rest = rest.split('#').next().unwrap_or_default();
    let (kind, rest) = rest.split_once('/').unwrap_or((rest, ""));
    match kind.to_ascii_lowercase().as_str() {
        "totp" => {}
        "hotp" => return Err("counter-based (HOTP) codes are not supported, only time-based ones".into()),
        _ => return Err("an otpauth:// link that is neither totp nor hotp".into()),
    }
    let (label, query) = rest.split_once('?').unwrap_or((rest, ""));
    let label = percent_decode(label, false);
    let (issuer, account) = match label.split_once(':') {
        Some((issuer, account)) => (issuer.trim(), account.trim()),
        None => ("", label.trim()),
    };
    let mut totp = Totp {
        secret: String::new(),
        digits: 6,
        period: 30,
        algorithm: "SHA1".into(),
        issuer: issuer.to_string(),
        label: account.to_string(),
    };
    let mut secret = None;
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let value = percent_decode(value, true);
        match key.to_ascii_lowercase().as_str() {
            "secret" => secret = Some(normalize_secret(&value)?),
            "issuer" if !value.trim().is_empty() => totp.issuer = value.trim().to_string(),
            "algorithm" => totp.algorithm = algorithm(&value)?.1.to_string(),
            "digits" => totp.digits = value.trim().parse().map_err(|_| "the link's digits are not a number")?,
            "period" => totp.period = value.trim().parse().map_err(|_| "the link's period is not a number")?,
            _ => {}
        }
    }
    totp.secret = secret.ok_or("the link has no secret")?;
    check(&totp)?;
    Ok(totp)
}

/// A bare secret as people paste it: base32, any case, spaces and dashes ignored, padding
/// optional. Six digits, thirty seconds, SHA1.
pub fn from_secret(secret: &str) -> Result<Totp, String> {
    Ok(Totp {
        secret: normalize_secret(secret)?,
        digits: 6,
        period: 30,
        algorithm: "SHA1".into(),
        issuer: String::new(),
        label: String::new(),
    })
}

/// The code at `unix_secs`, and the seconds it has left.
pub fn code(totp: &Totp, unix_secs: u64) -> Result<(String, u64), String> {
    check(totp)?;
    let (algorithm, _) = algorithm(&totp.algorithm)?;
    let key = decode_secret(&totp.secret)?;
    // RFC 6238: the counter is the number of whole periods since 1970, as 8 big-endian bytes.
    let counter = unix_secs / totp.period;
    let tag = hmac::sign(&hmac::Key::new(algorithm, &key), &counter.to_be_bytes());
    let tag = tag.as_ref();
    // RFC 4226's dynamic truncation: the low four bits of the last byte say where to take four
    // bytes from, less their top bit.
    let at = (tag[tag.len() - 1] & 0x0f) as usize;
    let number = u32::from_be_bytes([tag[at] & 0x7f, tag[at + 1], tag[at + 2], tag[at + 3]]);
    let code = number % 10u32.pow(totp.digits);
    let remaining = totp.period - unix_secs % totp.period;
    Ok((format!("{code:0width$}", width = totp.digits as usize), remaining))
}

/// What a code can be made from: 6 to 8 digits, a period of at least a second, and an algorithm
/// that is one of the three.
fn check(totp: &Totp) -> Result<(), String> {
    if !(6..=8).contains(&totp.digits) {
        return Err(format!("a code has 6 to 8 digits, not {}", totp.digits));
    }
    if totp.period == 0 {
        return Err("a code must last at least a second".into());
    }
    algorithm(&totp.algorithm)?;
    Ok(())
}

/// The HMAC for an algorithm's name, and the name as it is kept: SHA1 (also for an empty name,
/// the default), SHA256 or SHA512, in any case, with or without a dash.
fn algorithm(name: &str) -> Result<(hmac::Algorithm, &'static str), String> {
    match name.trim().to_ascii_uppercase().replace('-', "").as_str() {
        "SHA1" | "" => Ok((hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, "SHA1")),
        "SHA256" => Ok((hmac::HMAC_SHA256, "SHA256")),
        "SHA512" => Ok((hmac::HMAC_SHA512, "SHA512")),
        _ => Err(format!("\"{}\" is not SHA1, SHA256 or SHA512", name.trim())),
    }
}

/// A secret as people paste it, made what [`Totp::secret`] keeps: capitals, without the spaces,
/// dashes and padding. Refused if it is empty or not base32.
fn normalize_secret(text: &str) -> Result<String, String> {
    let mut secret = compact(text);
    decode_secret(&secret)?;
    Ok(std::mem::take(&mut *secret))
}

/// The secret's bytes. As forgiving as [`normalize_secret`], so that a secret kept some other
/// way still makes codes.
fn decode_secret(secret: &str) -> Result<Zeroizing<Vec<u8>>, String> {
    let text = compact(secret);
    if text.is_empty() {
        return Err("the secret is empty".into());
    }
    Ok(Zeroizing::new(base32().decode(text.as_bytes()).map_err(|_| "the secret is not base32")?))
}

/// `text` in capitals, without whitespace, dashes or padding. Made in a buffer large enough from
/// the start, so that growing it leaves no copy of a secret behind in a freed one.
fn compact(text: &str) -> Zeroizing<String> {
    let mut out = Zeroizing::new(String::with_capacity(text.len()));
    out.extend(text.chars().filter(|c| !c.is_whitespace() && !matches!(c, '-' | '=')).map(|c| c.to_ascii_uppercase()));
    out
}

/// RFC 4648 base32 without padding, forgiving unused bits at the end that are not zero: secrets
/// written out a character at a time rather than encoded from bytes have them, and
/// authenticator apps read them.
fn base32() -> &'static Encoding {
    static BASE32: OnceLock<Encoding> = OnceLock::new();
    BASE32.get_or_init(|| {
        let mut specification = BASE32_NOPAD.specification();
        specification.check_trailing_bits = false;
        specification.encoding().unwrap_or_else(|_| BASE32_NOPAD.clone())
    })
}

/// %XX escapes undone (and in a query, + as a space); a % that begins no escape is kept as it is.
fn percent_decode(text: &str, plus_is_space: bool) -> Zeroizing<String> {
    let bytes = text.as_bytes();
    let mut out = Zeroizing::new(Vec::with_capacity(bytes.len()));
    let mut i = 0;
    while i < bytes.len() {
        let hex = |at: usize| bytes.get(at).and_then(|&b| (b as char).to_digit(16));
        match (bytes[i], hex(i + 1), hex(i + 2)) {
            (b'%', Some(high), Some(low)) => {
                out.push((high * 16 + low) as u8);
                i += 3;
            }
            (b'+', _, _) if plus_is_space => {
                out.push(b' ');
                i += 1;
            }
            (b, _, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    Zeroizing::new(String::from_utf8_lossy(&out).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rfc_6238(seed: &[u8], algorithm: &str) -> Totp {
        Totp {
            secret: BASE32_NOPAD.encode(seed),
            digits: 8,
            period: 30,
            algorithm: algorithm.into(),
            ..Totp::default()
        }
    }

    #[test]
    fn makes_rfc_6238s_codes() {
        // RFC 6238 appendix B: its three seeds, 8 digits, 30 seconds.
        let sha1 = rfc_6238(b"12345678901234567890", "SHA1");
        let sha256 = rfc_6238(b"12345678901234567890123456789012", "SHA256");
        let sha512 = rfc_6238(b"1234567890123456789012345678901234567890123456789012345678901234", "SHA512");
        for (time, codes) in [
            (59, ["94287082", "46119246", "90693936"]),
            (1111111109, ["07081804", "68084774", "25091201"]),
            (1111111111, ["14050471", "67062674", "99943326"]),
            (1234567890, ["89005924", "91819424", "93441116"]),
            (2000000000, ["69279037", "90698825", "38618901"]),
            (20000000000, ["65353130", "77737706", "47863826"]),
        ] {
            for (totp, expected) in [&sha1, &sha256, &sha512].into_iter().zip(codes) {
                assert_eq!(code(totp, time).unwrap().0, expected, "{} at {time}", totp.algorithm);
            }
        }
    }

    #[test]
    fn says_how_long_a_code_has_left() {
        let totp = from_secret("JBSWY3DPEHPK3PXP").unwrap();
        assert_eq!(code(&totp, 0).unwrap(), ("282760".to_string(), 30));
        assert_eq!(code(&totp, 59).unwrap(), ("996554".to_string(), 1));
        assert_eq!(code(&totp, 60).unwrap().1, 30);
        assert_eq!(code(&totp, 1111111111).unwrap().1, 29);
        assert_eq!(code(&totp, 2000000000).unwrap().1, 10);
        let minute = Totp { period: 60, ..totp };
        assert_eq!(code(&minute, 59).unwrap().1, 1);
        assert_eq!(code(&minute, 61).unwrap().1, 59);
    }

    #[test]
    fn takes_a_secret_as_people_paste_it() {
        let totp = from_secret("jbsw y3dp-ehpk 3pxp====").unwrap();
        assert_eq!(totp.secret, "JBSWY3DPEHPK3PXP");
        assert_eq!((totp.digits, totp.period, totp.algorithm.as_str()), (6, 30, "SHA1"));
        assert_eq!(code(&totp, 1793241047).unwrap().0, "554988");
        // 26 characters are 130 bits, 16 bytes and two bits over, and H's last two bits are not
        // zero: strict base32 refuses it, authenticator apps do not.
        assert!(BASE32_NOPAD.decode(b"JBSWY3DPEHPK3PXPJBSWY3DPEH").is_err());
        let uneven = from_secret("JBSWY3DPEHPK3PXPJBSWY3DPEH").unwrap();
        assert_eq!(code(&uneven, 59).unwrap().0.len(), 6);
        for bad in ["", " - = ", "JBSWY3DPEHPK3PX1", "not base32!", "JBSWY3DPEHPK3PXPA"] {
            assert!(from_secret(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn refuses_what_makes_no_code() {
        let totp = from_secret("JBSWY3DPEHPK3PXP").unwrap();
        for bad in [
            Totp { digits: 5, ..totp.clone() },
            Totp { digits: 9, ..totp.clone() },
            Totp { period: 0, ..totp.clone() },
            Totp { algorithm: "MD5".into(), ..totp.clone() },
            Totp { secret: "1".into(), ..totp.clone() },
        ] {
            assert!(code(&bad, 59).is_err());
        }
        // Names as other apps write them.
        let sha256 = Totp { algorithm: "sha-256".into(), ..totp.clone() };
        let other = Totp { algorithm: "SHA256".into(), ..totp };
        assert_eq!(code(&sha256, 59).unwrap(), code(&other, 59).unwrap());
    }

    #[test]
    fn reads_googles_example_link() {
        let totp = parse_uri("otpauth://totp/Example:alice@google.com?secret=JBSWY3DPEHPK3PXP&issuer=Example").unwrap();
        assert_eq!(totp.issuer, "Example");
        assert_eq!(totp.label, "alice@google.com");
        assert_eq!(totp.secret, "JBSWY3DPEHPK3PXP");
        assert_eq!((totp.digits, totp.period, totp.algorithm.as_str()), (6, 30, "SHA1"));
        assert_eq!(code(&totp, 59).unwrap().0, "996554");
    }

    #[test]
    fn reads_every_parameter() {
        let totp = parse_uri(
            "otpauth://totp/ACME%20Co:john.doe@email.com?secret=HXDMVJECJJWSRB3HWIZR4IFUGFTMXBOZ&issuer=ACME%20Co&algorithm=SHA256&digits=8&period=60",
        )
        .unwrap();
        assert_eq!((totp.issuer.as_str(), totp.label.as_str()), ("ACME Co", "john.doe@email.com"));
        assert_eq!((totp.digits, totp.period, totp.algorithm.as_str()), (8, 60, "SHA256"));
        // The colon escaped, a space after it, the issuer only in the label, a lowercase secret.
        let totp = parse_uri("OTPAUTH://TOTP/Big%20Corp%3A%20paulus?SECRET=jbswy3dpehpk3pxp&image=x").unwrap();
        assert_eq!((totp.issuer.as_str(), totp.label.as_str()), ("Big Corp", "paulus"));
        assert_eq!(totp.secret, "JBSWY3DPEHPK3PXP");
        // No issuer anywhere; + as a space in the query; the parameter preferred to the label.
        let totp = parse_uri("otpauth://totp/paulus?secret=JBSWY3DPEHPK3PXP").unwrap();
        assert_eq!((totp.issuer.as_str(), totp.label.as_str()), ("", "paulus"));
        let totp = parse_uri("otpauth://totp/Old:paulus?secret=JBSWY3DPEHPK3PXP&issuer=New+Name").unwrap();
        assert_eq!(totp.issuer, "New Name");
    }

    #[test]
    fn refuses_links_it_cannot_use() {
        for bad in [
            "otpauth://hotp/Example:alice?secret=JBSWY3DPEHPK3PXP&counter=0",
            "https://example.com/totp?secret=JBSWY3DPEHPK3PXP",
            "otpauth://totp/Example:alice?issuer=Example",
            "otpauth://totp/Example:alice?secret=JBSWY3DPEHPK3PX1",
            "otpauth://totp/Example:alice?secret=JBSWY3DPEHPK3PXP&digits=10",
            "otpauth://totp/Example:alice?secret=JBSWY3DPEHPK3PXP&digits=six",
            "otpauth://totp/Example:alice?secret=JBSWY3DPEHPK3PXP&period=0",
            "otpauth://totp/Example:alice?secret=JBSWY3DPEHPK3PXP&algorithm=MD5",
            "otpauth://steam/Example:alice?secret=JBSWY3DPEHPK3PXP",
            "otpauth:/",
        ] {
            let refusal = parse_uri(bad).err().unwrap_or_else(|| panic!("read {bad}"));
            assert!(!refusal.contains("JBSWY3DPEHPK3P"), "the secret in the error: {refusal}");
        }
        assert!(parse_uri("otpauth://hotp/x?secret=JBSWY3DPEHPK3PXP").unwrap_err().contains("HOTP"));
    }

    #[test]
    fn percent_escapes_come_undone() {
        assert_eq!(percent_decode("a%20b%3Ac%3a%E2%82%AC", false).as_str(), "a b:c:\u{20ac}");
        assert_eq!(percent_decode("100%+%zz%4", false).as_str(), "100%+%zz%4");
        assert_eq!(percent_decode("a+b", true).as_str(), "a b");
    }
}
