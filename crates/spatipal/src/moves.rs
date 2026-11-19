//! A pal's moves made from words (the owner, 2026-11-19, of hemera's motion: "build the Spatiand
//! side"; "make sure the function is polished"): its model asks for a move in English -- how the
//! body moves, for how long, looped for an idle -- hemera makes it (`spatiand_voice::motion`), and
//! the room plays it on the pal's own figure (`crates/spatiand/src/motion.rs`).
//!
//! Each move made is kept in the pal's folder (`moves/<name>.json`: the words, the seed, the
//! motion), so that it is done again by its name with `appearance`'s `move`, not made again. One
//! made as an idle loops, and is kept as `idle`: the room plays it whenever the pal is idle, over
//! its figure's own.

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::time::Instant;

use serde_json::{json, Value};
use spatiand_voice::motion::{self, Ask, Shelf};
use spatiand_voice::Hemera;

/// The tool its models are given for it.
pub fn tool() -> Value {
    spatiand_voice::chat::tool(
        "make_move",
        "Make a new move of your own from words, and do it, whenever the wearer asks for a move you do not have -- anything a person's \
         body can do: a dance, a cartwheel, a bow, a jump, a stretch, a stroll -- or you want one. A new idle -- how you stand and wait \
         while nothing happens -- is kind idle. It takes a few seconds to make (longer the first time): say something meanwhile. Once made it \
         is yours to do again by its name, with appearance's move.",
        json!({
            "words": {"type": "string", "description": "How your body moves, in English, as one would describe a person moving -- one or two actions \
                (more are followed loosely), a sentence: \"a person does a cartwheel\", \"a person spins around once with the arms held out\", \"a person \
                sways the hips slowly from side to side with both arms raised\". Only the movement: no feelings, clothes, objects or other people."},
            "name": {"type": "string", "description": "A short name to keep it by: \"twirl\", \"slow_sway\""},
            "seconds": {"type": "number", "description": "How long it lasts, 1 to 10 seconds; 3 or 4 suit most moves"},
            "kind": {"type": "string", "enum": ["move", "idle"], "description": "move: done once, now; idle: a quiet loop -- how you stand while nothing \
                else happens -- yours from now on whenever you are idle"}
        }),
        &["words", "name", "kind"],
    )
}

/// A call of the tool checked: what hemera is asked, and the name it is kept by -- or why not, in
/// words its model can mend the call by.
pub fn asked(args: &Value) -> Result<(Ask, String), String> {
    let words = args.get("words").and_then(Value::as_str).map(str::trim).unwrap_or_default();
    if words.is_empty() {
        return Err("say in English how your body moves: \"a person spins around once with the arms held out\"".into());
    }
    if words.split_whitespace().count() > 60 {
        return Err("say it in at most sixty words: one or two actions".into());
    }
    let idle = args.get("kind").and_then(Value::as_str).is_some_and(|k| k.trim().eq_ignore_ascii_case("idle")) || args.get("idle").and_then(Value::as_bool).unwrap_or(false);
    let name = if idle { "idle".to_string() } else { motion::name_of(args.get("name").and_then(Value::as_str).unwrap_or(words)) };
    if name.is_empty() {
        return Err("give the move a short name to keep it by".into());
    }
    let seconds = args.get("seconds").and_then(Value::as_f64).map(|s| s as f32).unwrap_or(if idle { 4.0 } else { 3.0 });
    Ok((Ask { prompt: words.to_string(), duration: seconds.clamp(1.0, 10.0), looping: idle, seed: None }, name))
}

/// A move being made, on a thread of its own.
pub struct Making {
    pub name: String,
    pub since: Instant,
    rx: Receiver<Result<PathBuf, String>>,
}

impl Making {
    /// Made and kept (its file), failed (why), or `None` while it is still being made.
    pub fn done(&self) -> Option<Result<PathBuf, String>> {
        match self.rx.try_recv() {
            Ok(r) => Some(r),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err("the move's making stopped".into())),
        }
    }
}

/// Start making a move: asked of hemera at `hemera`, kept on `shelf` (a pal's `moves/`) by `name`.
pub fn start(hemera: &str, ask: Ask, name: String, shelf: PathBuf) -> Making {
    let (tx, rx) = channel();
    let (base, kept_as) = (hemera.to_string(), name.clone());
    std::thread::Builder::new()
        .name("move".into())
        .spawn(move || {
            let made = motion::make(&Hemera::new(&base), &ask, |_| {}).and_then(|m| Shelf { dir: shelf }.keep(&kept_as, &ask, &m));
            let _ = tx.send(made);
        })
        .expect("a thread for a move");
    Making { name, since: Instant::now(), rx }
}

/// What the tool answers at once, while the move is made: that it is not done yet -- a small
/// model, told only to say something meanwhile, said "there you go" the moment it asked
/// (2026-11-19), and the move came 45 s later, hemera having had to load its model first.
pub fn begun(name: &str, looping: bool) -> String {
    if looping {
        format!(
            "Your new idle ({name}) is being made: it is not done yet, and takes a few seconds -- up to a minute the first time. Say you are \
             working it out; from when it is made you stand in it whenever you are idle."
        )
    } else {
        format!(
            "The move {name} is being made: it is not done yet. It takes a few seconds -- up to a minute the first time -- and then you do it. \
             Say you are working it out, not that it is done. Afterwards it is yours to do again (appearance, move {name})."
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tool_says_what_hemera_understands() {
        let t = tool();
        assert_eq!(t["function"]["name"], "make_move");
        let props = &t["function"]["parameters"]["properties"];
        assert!(props["words"]["description"].as_str().unwrap().contains("English"));
        assert_eq!(t["function"]["parameters"]["required"], json!(["words", "name", "kind"]));
    }

    #[test]
    fn a_call_is_checked_and_named() {
        let (ask, name) = asked(&json!({"words": "a person spins around once with the arms held out", "name": "Twirl!", "seconds": 30})).unwrap();
        assert_eq!(name, "twirl");
        assert_eq!(ask.duration, 10.0);
        assert!(!ask.looping);
        // An idle is a loop, kept as `idle`: the room's own name for it.
        let (ask, name) = asked(&json!({"words": "a person stands and sways gently", "name": "calm", "kind": "idle"})).unwrap();
        assert_eq!((name.as_str(), ask.looping, ask.duration), ("idle", true, 4.0));
        assert!(asked(&json!({"words": "a person stands and sways gently", "name": "calm", "idle": true})).unwrap().0.looping);
        assert!(!asked(&json!({"words": "a person waves", "name": "w", "kind": "move"})).unwrap().0.looping);
        // No words, too many, or no name left: said how to mend it.
        assert!(asked(&json!({"name": "x"})).unwrap_err().contains("English"));
        assert!(asked(&json!({"words": "move ".repeat(61), "name": "x"})).unwrap_err().contains("sixty"));
        assert!(asked(&json!({"words": "a person waves", "name": "!!!"})).unwrap_err().contains("name"));
        // A name left out is made from the words.
        assert_eq!(asked(&json!({"words": "a person waves"})).unwrap().1, "a_person_waves");
    }

    #[test]
    fn a_move_that_cannot_be_made_says_why() {
        let dir = std::env::temp_dir().join(format!("spatipal-moves-test-{}", std::process::id()));
        let (ask, name) = asked(&json!({"words": "a person waves", "name": "wave"})).unwrap();
        // Nothing listens there.
        let making = start("http://127.0.0.1:9", ask, name, dir.clone());
        let since = Instant::now();
        let done = loop {
            if let Some(d) = making.done() {
                break d;
            }
            assert!(since.elapsed().as_secs() < 10, "no answer");
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        assert!(done.unwrap_err().contains("could not be reached"));
        assert!(!dir.join("wave.json").exists());
        assert!(begun("twirl", false).contains("appearance, move twirl"));
        assert!(begun("twirl", false).contains("not done yet"));
    }
}
