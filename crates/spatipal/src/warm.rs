//! Keeping SpatiPal's models ready on hemera.
//!
//! hemera starts a model when it is first asked for and keeps it after, but nothing asked for
//! them before the wearer did: on 2026-10-29 the first "Pixel, how are you?" waited while Whisper,
//! the small model and the voice were each started in turn -- 10.5 s in the small model alone.
//! So when SpatiPal starts listening, it asks hemera to start what it will need, and has the small
//! model read what every request begins with (its tools and system message), which it then keeps:
//! the first answer finds it read. Looked at again, at most every few minutes, when someone starts
//! speaking -- a model stopped meanwhile starts while they talk.
//!
//! The larger model is never started here: it is big, and seldom needed. When something else has
//! started it, it is given the start of its requests to read too -- some eight seconds' work for
//! it, done before the wearer waits on it.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use spatiand_voice::Hemera;

/// What to have ready.
pub struct Wanted {
    /// Models to have started: Whisper, the voice.
    pub models: Vec<String>,
    /// The small model, and a request that begins as every request to it does.
    pub small: String,
    pub prime: Value,
    /// The larger model, and the same for it: read only if it is up already.
    pub large: String,
    pub prime_large: Value,
}

/// Not looked at again sooner than this.
pub const EVERY: Duration = Duration::from_secs(180);

/// Have `wanted` ready, on a thread of its own. `prime` also when the small model is up already:
/// what it read last may have been something else.
pub fn warm(hemera: Hemera, wanted: Wanted, prime: bool) {
    let spawned = std::thread::Builder::new().name("warm".into()).spawn(move || {
        let status = match hemera.get_json("/hemera/api/status") {
            Ok(s) => s,
            Err(why) => {
                log::info!("warm: hemera's state is unknown: {why}");
                Value::Null
            }
        };
        let up = |name: &str| {
            let named = |list: &str| {
                status.get(list).and_then(Value::as_array).into_iter().flatten().any(|m| m.as_str().or_else(|| m.get("name").and_then(Value::as_str)) == Some(name))
            };
            named("instances") || named("loading")
        };
        for model in wanted.models.iter().filter(|m| !m.is_empty() && !up(m)) {
            match hemera.post_json("/hemera/api/models/load", &json!({ "name": model }), Duration::from_secs(10)) {
                Ok(_) => log::info!("warm: hemera is starting {model}"),
                Err(why) => log::warn!("warm: {model} could not be started: {why}"),
            }
        }
        // Asked for, the small model is started if it is not; and each keeps what it read.
        let read = |model: &str, request: &Value| {
            let started = Instant::now();
            match hemera.post_json("/v1/chat/completions", request, Duration::from_secs(180)) {
                Ok(reply) => {
                    let tokens = reply.pointer("/timings/prompt_n").and_then(Value::as_u64).unwrap_or(0);
                    log::info!("warm: {model} read the start of every request ({tokens} tokens) in {:.1} s", started.elapsed().as_secs_f32());
                }
                Err(why) => log::warn!("warm: {model} did not answer: {why}"),
            }
        };
        if !wanted.small.is_empty() && (prime || !up(&wanted.small)) {
            read(&wanted.small, &wanted.prime);
        }
        if prime && !wanted.large.is_empty() && wanted.large != wanted.small && up(&wanted.large) {
            read(&wanted.large, &wanted.prime_large);
        }
    });
    if let Err(e) = spawned {
        log::warn!("warm: no thread: {e}");
    }
}
