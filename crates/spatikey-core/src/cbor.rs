//! Just enough CBOR (RFC 8949) for WebAuthn: writing attestation objects and COSE keys, and
//! reading them back in tests. Canonical: map keys are written in the order given, which the
//! callers give in CTAP2's canonical order.

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Int(i64),
    Bytes(Vec<u8>),
    Text(String),
    Array(Vec<Value>),
    Map(Vec<(Value, Value)>),
    Bool(bool),
    Null,
}

impl Value {
    /// In a map, the value under `key`.
    pub fn get(&self, key: &Value) -> Option<&Value> {
        match self {
            Value::Map(pairs) => pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_int(&self) -> Option<i64> {
        match self {
            Value::Int(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Value::Bytes(bytes) => Some(bytes),
            _ => None,
        }
    }

    pub fn as_text(&self) -> Option<&str> {
        match self {
            Value::Text(text) => Some(text),
            _ => None,
        }
    }
}

// The major types: the top three bits of a data item's first byte.
const UNSIGNED: u8 = 0;
const NEGATIVE: u8 = 1;
const BYTES: u8 = 2;
const TEXT: u8 = 3;
const ARRAY: u8 = 4;
const MAP: u8 = 5;
const TAG: u8 = 6;
const SIMPLE: u8 = 7;

/// How deep arrays and maps may nest when read. An attestation object goes three deep; the
/// limit only keeps a hostile input from using up the stack.
const MAX_DEPTH: usize = 16;

/// Write `value` with every number and length in its shortest form, as CTAP2's canonical CBOR
/// asks, and definite lengths only.
pub fn encode(value: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    write(&mut out, value);
    out
}

fn write(out: &mut Vec<u8>, value: &Value) {
    match value {
        // A negative n is written as -1 - n, which for an i64 is its bitwise complement and so
        // never overflows, i64::MIN included.
        Value::Int(n) if *n < 0 => head(out, NEGATIVE, (!*n) as u64),
        Value::Int(n) => head(out, UNSIGNED, *n as u64),
        Value::Bytes(bytes) => {
            head(out, BYTES, bytes.len() as u64);
            out.extend_from_slice(bytes);
        }
        Value::Text(text) => {
            head(out, TEXT, text.len() as u64);
            out.extend_from_slice(text.as_bytes());
        }
        Value::Array(items) => {
            head(out, ARRAY, items.len() as u64);
            for item in items {
                write(out, item);
            }
        }
        Value::Map(pairs) => {
            head(out, MAP, pairs.len() as u64);
            for (key, value) in pairs {
                write(out, key);
                write(out, value);
            }
        }
        Value::Bool(false) => out.push(SIMPLE << 5 | 20),
        Value::Bool(true) => out.push(SIMPLE << 5 | 21),
        Value::Null => out.push(SIMPLE << 5 | 22),
    }
}

/// A data item's head: the major type, and its argument (the number itself, or how many bytes,
/// items or pairs follow) in the fewest bytes that hold it.
fn head(out: &mut Vec<u8>, major: u8, argument: u64) {
    let major = major << 5;
    if argument < 24 {
        out.push(major | argument as u8);
    } else if argument <= u8::MAX as u64 {
        out.push(major | 24);
        out.push(argument as u8);
    } else if argument <= u16::MAX as u64 {
        out.push(major | 25);
        out.extend_from_slice(&(argument as u16).to_be_bytes());
    } else if argument <= u32::MAX as u64 {
        out.push(major | 26);
        out.extend_from_slice(&(argument as u32).to_be_bytes());
    } else {
        out.push(major | 27);
        out.extend_from_slice(&argument.to_be_bytes());
    }
}

/// Read one value from the front of `bytes`, returning it and what is left. Only what
/// [`encode`] writes is read back: a number or length not in its shortest form, an indefinite
/// length, a tag, a float or a simple value other than true, false and null is an error, as is a
/// value cut short.
pub fn decode(bytes: &[u8]) -> Result<(Value, &[u8]), String> {
    read(bytes, 0)
}

fn read(bytes: &[u8], depth: usize) -> Result<(Value, &[u8]), String> {
    let (&first, rest) = bytes.split_first().ok_or("cut short")?;
    let major = first >> 5;
    let info = first & 0x1f;
    if major == SIMPLE {
        return match info {
            20 => Ok((Value::Bool(false), rest)),
            21 => Ok((Value::Bool(true), rest)),
            22 => Ok((Value::Null, rest)),
            25..=27 => Err("a float is not read".into()),
            _ => Err(format!("the simple value {info} is not read")),
        };
    }
    if major == TAG {
        return Err("a tag is not read".into());
    }
    let (argument, rest) = argument(info, rest)?;
    match major {
        UNSIGNED => {
            let n = i64::try_from(argument).map_err(|_| "an integer too large to read")?;
            Ok((Value::Int(n), rest))
        }
        NEGATIVE => {
            // -1 - argument, the complement of what encode wrote.
            let n = i64::try_from(argument).map_err(|_| "an integer too small to read")?;
            Ok((Value::Int(!n), rest))
        }
        BYTES => {
            let (body, rest) = take(rest, argument)?;
            Ok((Value::Bytes(body.to_vec()), rest))
        }
        TEXT => {
            let (body, rest) = take(rest, argument)?;
            let text = std::str::from_utf8(body).map_err(|_| "text that is not UTF-8")?;
            Ok((Value::Text(text.to_string()), rest))
        }
        ARRAY | MAP => {
            if depth >= MAX_DEPTH {
                return Err("arrays or maps nested too deep".into());
            }
            // Each item takes at least a byte, so a count larger than what is left runs out of
            // bytes in the loop rather than being allocated for up front.
            let mut rest = rest;
            if major == ARRAY {
                let mut items = Vec::new();
                for _ in 0..argument {
                    let (item, after) = read(rest, depth + 1)?;
                    items.push(item);
                    rest = after;
                }
                Ok((Value::Array(items), rest))
            } else {
                let mut pairs = Vec::new();
                for _ in 0..argument {
                    let (key, after) = read(rest, depth + 1)?;
                    let (value, after) = read(after, depth + 1)?;
                    pairs.push((key, value));
                    rest = after;
                }
                Ok((Value::Map(pairs), rest))
            }
        }
        _ => unreachable!("a major type is three bits, and 6 and 7 are answered above"),
    }
}

/// A head's argument: the additional information itself below 24, else the 1, 2, 4 or 8 bytes
/// after it. One that would have fitted a shorter form is refused, as is an indefinite length.
fn argument(info: u8, bytes: &[u8]) -> Result<(u64, &[u8]), String> {
    let size = match info {
        0..=23 => return Ok((info as u64, bytes)),
        24 => 1,
        25 => 2,
        26 => 4,
        27 => 8,
        31 => return Err("an indefinite length is not read".into()),
        _ => return Err(format!("the reserved additional information {info}")),
    };
    let (field, rest) = take(bytes, size)?;
    let value = field.iter().fold(0u64, |n, &b| (n << 8) | b as u64);
    let shortest = match size {
        1 => value >= 24,
        2 => value > u8::MAX as u64,
        4 => value > u16::MAX as u64,
        _ => value > u32::MAX as u64,
    };
    if !shortest {
        return Err("a number not in its shortest form".into());
    }
    Ok((value, rest))
}

/// The first `count` bytes and the rest, or an error if there are not that many.
fn take(bytes: &[u8], count: u64) -> Result<(&[u8], &[u8]), String> {
    let count = usize::try_from(count).ok().filter(|&count| count <= bytes.len()).ok_or("cut short")?;
    Ok(bytes.split_at(count))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str) -> Value {
        Value::Text(s.into())
    }

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    /// RFC 8949's appendix A, for the kinds of value read and written here.
    fn examples() -> Vec<(Value, &'static str)> {
        vec![
            (Value::Int(0), "00"),
            (Value::Int(1), "01"),
            (Value::Int(10), "0a"),
            (Value::Int(23), "17"),
            (Value::Int(24), "1818"),
            (Value::Int(25), "1819"),
            (Value::Int(100), "1864"),
            (Value::Int(1000), "1903e8"),
            (Value::Int(1000000), "1a000f4240"),
            (Value::Int(1000000000000), "1b000000e8d4a51000"),
            (Value::Int(i64::MAX), "1b7fffffffffffffff"),
            (Value::Int(-1), "20"),
            (Value::Int(-10), "29"),
            (Value::Int(-24), "37"),
            (Value::Int(-25), "3818"),
            (Value::Int(-100), "3863"),
            (Value::Int(-1000), "3903e7"),
            (Value::Int(i64::MIN), "3b7fffffffffffffff"),
            (Value::Bool(false), "f4"),
            (Value::Bool(true), "f5"),
            (Value::Null, "f6"),
            (Value::Bytes(vec![]), "40"),
            (Value::Bytes(vec![1, 2, 3, 4]), "4401020304"),
            (text(""), "60"),
            (text("a"), "6161"),
            (text("IETF"), "6449455446"),
            (text("\"\\"), "62225c"),
            (text("\u{fc}"), "62c3bc"),
            (text("\u{6c34}"), "63e6b0b4"),
            (Value::Array(vec![]), "80"),
            (Value::Array(vec![Value::Int(1), Value::Int(2), Value::Int(3)]), "83010203"),
            (
                Value::Array(vec![
                    Value::Int(1),
                    Value::Array(vec![Value::Int(2), Value::Int(3)]),
                    Value::Array(vec![Value::Int(4), Value::Int(5)]),
                ]),
                "8301820203820405",
            ),
            (
                Value::Array((1..=25).map(Value::Int).collect()),
                "98190102030405060708090a0b0c0d0e0f101112131415161718181819",
            ),
            (Value::Map(vec![]), "a0"),
            (Value::Map(vec![(Value::Int(1), Value::Int(2)), (Value::Int(3), Value::Int(4))]), "a201020304"),
            (
                Value::Map(vec![
                    (text("a"), Value::Int(1)),
                    (text("b"), Value::Array(vec![Value::Int(2), Value::Int(3)])),
                ]),
                "a26161016162820203",
            ),
        ]
    }

    #[test]
    fn writes_rfc_8949s_examples() {
        for (value, expected) in examples() {
            assert_eq!(encode(&value), hex(expected), "{value:?}");
        }
    }

    #[test]
    fn reads_back_what_it_writes() {
        for (value, encoded) in examples() {
            let bytes = hex(encoded);
            let (read, rest) = decode(&bytes).unwrap();
            assert_eq!(read, value);
            assert!(rest.is_empty());
        }
        let long = Value::Bytes(vec![7; 70000]);
        let bytes = encode(&long);
        assert_eq!(&bytes[..5], [0x5a, 0x00, 0x01, 0x11, 0x70]);
        assert_eq!(decode(&bytes).unwrap().0, long);
    }

    #[test]
    fn reading_leaves_what_follows() {
        let (value, rest) = decode(&[0x18, 0x64, 0xf6, 0x01]).unwrap();
        assert_eq!(value, Value::Int(100));
        assert_eq!(rest, [0xf6, 0x01]);
    }

    #[test]
    fn refuses_what_is_cut_short() {
        for bytes in [
            &[][..],
            &[0x18],
            &[0x19, 0x03],
            &[0x1b, 0, 0, 0, 0, 0, 0, 1],
            &[0x44, 1, 2, 3],
            &[0x63, b'a', b'b'],
            &[0x82, 0x01],
            &[0xa1, 0x01],
            &[0x9b, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01],
        ] {
            assert!(decode(bytes).is_err(), "{bytes:02x?}");
        }
    }

    #[test]
    fn refuses_what_it_would_not_write() {
        for bytes in [
            // Not the shortest form.
            &[0x18, 0x05][..],
            &[0x19, 0x00, 0xff],
            &[0x1a, 0x00, 0x00, 0xff, 0xff],
            &[0x1b, 0, 0, 0, 0, 0xff, 0xff, 0xff, 0xff],
            &[0x38, 0x00],
            &[0x58, 0x01, 0xaa],
            // Indefinite lengths, and the reserved additional information.
            &[0x5f, 0x41, 0x00, 0xff],
            &[0x9f, 0xff],
            &[0x1c],
            // Tags, floats, undefined.
            &[0xc0, 0x60],
            &[0xf9, 0x3c, 0x00],
            &[0xf7],
            // Integers beyond an i64.
            &[0x1b, 0x80, 0, 0, 0, 0, 0, 0, 0],
            &[0x3b, 0x80, 0, 0, 0, 0, 0, 0, 0],
            // Text that is not UTF-8.
            &[0x62, 0xc3, 0x28],
        ] {
            assert!(decode(bytes).is_err(), "{bytes:02x?}");
        }
    }

    #[test]
    fn refuses_nesting_past_the_limit() {
        let mut deep = vec![0x81; 100];
        deep.push(0x00);
        assert!(decode(&deep).is_err());
        let mut fine = vec![0x81; MAX_DEPTH];
        fine.push(0x00);
        assert!(decode(&fine).is_ok());
    }

    #[test]
    fn finds_a_maps_values() {
        let map = Value::Map(vec![(Value::Int(-2), Value::Bytes(vec![9])), (text("fmt"), text("none"))]);
        assert_eq!(map.get(&Value::Int(-2)).and_then(Value::as_bytes), Some(&[9u8][..]));
        assert_eq!(map.get(&text("fmt")).and_then(Value::as_text), Some("none"));
        assert_eq!(map.get(&Value::Int(1)), None);
        assert_eq!(Value::Int(3).get(&Value::Int(3)), None);
        assert_eq!(Value::Int(-7).as_int(), Some(-7));
    }
}
