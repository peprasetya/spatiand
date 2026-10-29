//! Just enough CBOR (RFC 8949) for WebAuthn: writing attestation objects and COSE keys, and
//! reading them back in tests. Canonical: map keys are written in the order given, which the
//! callers give in CTAP2's canonical order.
//!
//! STUB: the signatures are the contract; the bodies are to be written.

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

pub fn encode(value: &Value) -> Vec<u8> {
    let _ = value;
    todo!()
}

/// Read one value from the front of `bytes`, returning it and what is left.
pub fn decode(bytes: &[u8]) -> Result<(Value, &[u8]), String> {
    let _ = bytes;
    todo!()
}
