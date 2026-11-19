//! A pal's moves made from words (the owner, 2026-11-19, of hemera's motion: "build the Spatiand
//! side"; "make sure the function is polished"): its model asks for a move in English -- how the
//! body moves, for how long -- hemera makes it (`spatiand_voice::motion`), and the room plays it on
//! the pal's own figure (`crates/spatiand/src/motion.rs`).
//!
//! What it is made for (`kind`): a **move**, done once and kept with the pal's moves (`moves/`); a
//! **pose**, a quiet loop held until another pose, kept with its poses (`poses/`) -- the small
//! model's way to any pose its figure has no name for; an **idle**, a loop kept as `moves/idle`,
//! which the room then plays whenever the pal is idle. Each is kept in the pal's folder with the
//! words and seed it was made from, so that it is done again by its name with `appearance`, not
//! made again; `forget` lets one go.

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
        "Make a new move or pose of your own from words, and do it, whenever the wearer asks you to do something with your body that is not \
         one of appearance's moves or poses -- anything a person's body can do: a dance, a cartwheel, a backflip, a bow, standing on one \
         leg, a stretch -- or you want one. Never say you cannot do a move: make it. It takes a few seconds (up to a minute the first time). \
         Once made it is yours to do again by its name, with appearance.",
        json!({
            "words": {"type": "string", "description": "How your body moves or stands, in English, as one would describe a person -- one or two actions \
                (more are followed loosely), a sentence: \"a person does a cartwheel\", \"a person stands on one leg with the arms out for balance\", \
                \"a person sways the hips slowly from side to side with both arms raised\". Only the body: no feelings, clothes, objects or other people."},
            "name": {"type": "string", "description": "A short name to keep it by: \"cartwheel\", \"one_leg\""},
            "seconds": {"type": "number", "description": "How long it lasts, 1 to 10 seconds; 3 or 4 suit most"},
            "kind": {"type": "string", "enum": ["move", "pose", "idle", "forget"], "description": "move: done once, now, and then you stand as before \
                (a dance, a jump, a bow); pose: a position you stay in until you choose another (\"kneel on one knee\", \"stand on one leg\", \"sit \
                cross-legged\"), its words saying how you are while in it, not how you get there; idle: a quiet loop, how you stand while nothing else \
                happens, yours from now on whenever you are idle; forget: let go of a move or pose you made, by its name (no words needed)"}
        }),
        &["words", "name", "kind"],
    )
}

/// What a move made is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Done once, now; kept with its moves.
    Move,
    /// Held until another pose; kept with its poses.
    Pose,
    /// A quiet loop, the pal's idle from now on.
    Idle,
}

impl Kind {
    /// What it is called, in what is told and logged.
    pub fn word(self) -> &'static str {
        match self {
            Kind::Move => "move",
            Kind::Pose => "pose",
            Kind::Idle => "idle",
        }
    }
}

/// What a call of the tool asks.
#[derive(Debug, PartialEq)]
pub enum Asked {
    /// Something to make: what hemera is asked, the name it is kept by, what for.
    Make { ask: Ask, name: String, kind: Kind },
    /// One of its own to let go of, by its name.
    Forget(String),
}

/// A call of the tool checked -- or why not, in words its model can mend the call by.
pub fn asked(args: &Value) -> Result<Asked, String> {
    let kind = args.get("kind").and_then(Value::as_str).map(|k| k.trim().to_lowercase()).unwrap_or_default();
    if kind == "forget" {
        let name = motion::name_of(args.get("name").and_then(Value::as_str).unwrap_or_default());
        return if name.is_empty() { Err("say which move to forget, by its name".into()) } else { Ok(Asked::Forget(name)) };
    }
    let words = args.get("words").and_then(Value::as_str).map(str::trim).unwrap_or_default();
    if words.is_empty() {
        return Err("say in English how your body moves: \"a person spins around once with the arms held out\"".into());
    }
    if words.split_whitespace().count() > 60 {
        return Err("say it in at most sixty words: one or two actions".into());
    }
    let kind = match kind.as_str() {
        "idle" => Kind::Idle,
        "pose" => Kind::Pose,
        // An older call's `idle: true`.
        _ if args.get("idle").and_then(Value::as_bool) == Some(true) => Kind::Idle,
        _ => Kind::Move,
    };
    let name = match kind {
        Kind::Idle => "idle".to_string(),
        _ => motion::name_of(args.get("name").and_then(Value::as_str).unwrap_or(words)),
    };
    if name.is_empty() {
        return Err("give it a short name to keep it by".into());
    }
    let seconds = args.get("seconds").and_then(Value::as_f64).map(|s| s as f32).unwrap_or(if kind == Kind::Move { 3.0 } else { 4.0 });
    let ask = Ask { prompt: words.to_string(), duration: seconds.clamp(1.0, 10.0), looping: kind != Kind::Move, seed: None };
    Ok(Asked::Make { ask, name, kind })
}

/// A move being made, on a thread of its own.
pub struct Making {
    pub name: String,
    pub kind: Kind,
    pub since: Instant,
    /// The pal's folder it is kept in: another pal may be in the room by the time it is made.
    pub pal: PathBuf,
    /// What the pal held when it was asked for (`Look::holding`): a pose made is taken only if
    /// it holds the same still, not over one taken meanwhile.
    pub held_then: String,
    /// Let go of while it was being made: not kept once made.
    pub forget: bool,
    /// Taken when made (a pose): not when another pose or move was asked for after it.
    pub take: bool,
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

/// Start making it: asked of hemera at `hemera`, kept on `shelf` (a folder in the pal's) by
/// `name`.
pub fn start(hemera: &str, ask: Ask, name: String, kind: Kind, shelf: PathBuf) -> Making {
    let (tx, rx) = channel();
    let (base, kept_as) = (hemera.to_string(), name.clone());
    let pal = shelf.parent().map(PathBuf::from).unwrap_or_default();
    std::thread::Builder::new()
        .name("move".into())
        .spawn(move || {
            let made = motion::make(&Hemera::new(&base), &ask, |_| {}).and_then(|m| Shelf { dir: shelf }.keep(&kept_as, &ask, &m));
            let _ = tx.send(made);
        })
        .expect("a thread for a move");
    Making { name, kind, since: Instant::now(), pal, held_then: String::new(), forget: false, take: true, rx }
}

/// What the tool answers at once, while it is made: that it is not done yet, and that it will
/// do it by itself -- a small model, told only to say something meanwhile, said "there you go"
/// the moment it asked (2026-11-19), and the move came 45 s later, hemera having had to load its
/// model first; told "Then you do it", both models still did it in words at once ("I'm standing
/// on one leg now", "... there! Did you see that?", 2026-11-20).
pub fn begun(name: &str, kind: Kind) -> String {
    let (what, when) = match kind {
        Kind::Move => (format!("the move {name}"), "you do it by yourself the moment it is ready"),
        Kind::Pose => (format!("the pose {name}"), "you take it by yourself the moment it is ready"),
        Kind::Idle => ("your new idle".to_string(), "from then on you stand in it whenever you are idle"),
    };
    let after = match kind {
        Kind::Move => format!(" Once made it is yours to do again (appearance, move {name})."),
        Kind::Pose => format!(" Once made it is yours to take again (appearance, pose {name})."),
        Kind::Idle => String::new(),
    };
    format!(
        "Not ready yet: {what} is being made. It comes in a few seconds -- up to a minute the first time -- and {when}. So it has not \
         happened yet: answer in character that you are about to, without acting it out or saying you are doing it or did it.{after}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn made(args: Value) -> (Ask, String, Kind) {
        match asked(&args) {
            Ok(Asked::Make { ask, name, kind }) => (ask, name, kind),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_tool_says_what_hemera_understands() {
        let t = tool();
        assert_eq!(t["function"]["name"], "make_move");
        let props = &t["function"]["parameters"]["properties"];
        assert!(props["words"]["description"].as_str().unwrap().contains("English"));
        assert_eq!(props["kind"]["enum"], json!(["move", "pose", "idle", "forget"]));
        assert_eq!(t["function"]["parameters"]["required"], json!(["words", "name", "kind"]));
    }

    #[test]
    fn a_call_is_checked_named_and_kinded() {
        let (ask, name, kind) = made(json!({"words": "a person spins around once with the arms held out", "name": "Twirl!", "seconds": 30, "kind": "move"}));
        assert_eq!((name.as_str(), kind, ask.duration, ask.looping), ("twirl", Kind::Move, 10.0, false));
        // An idle is a loop, kept as `idle`: the room's own name for it.
        let (ask, name, kind) = made(json!({"words": "a person stands and sways gently", "name": "calm", "kind": "idle"}));
        assert_eq!((name.as_str(), kind, ask.looping, ask.duration), ("idle", Kind::Idle, true, 4.0));
        assert_eq!(made(json!({"words": "a person sways", "name": "calm", "idle": true})).2, Kind::Idle);
        // A pose is a loop too, held, by its own name.
        let (ask, name, kind) = made(json!({"words": "a person stands on one leg", "name": "One Leg", "kind": "pose"}));
        assert_eq!((name.as_str(), kind, ask.looping), ("one_leg", Kind::Pose, true));
        // Let go of by its name; no words needed.
        assert_eq!(asked(&json!({"kind": "forget", "name": "Twirl"})), Ok(Asked::Forget("twirl".into())));
        assert!(asked(&json!({"kind": "forget"})).is_err());
        // No words, too many, or no name left: said how to mend it.
        assert!(asked(&json!({"name": "x", "kind": "move"})).unwrap_err().contains("English"));
        assert!(asked(&json!({"words": "move ".repeat(61), "name": "x"})).unwrap_err().contains("sixty"));
        assert!(asked(&json!({"words": "a person waves", "name": "!!!"})).unwrap_err().contains("name"));
        // A name left out is made from the words, and a kind left out is a move.
        assert_eq!(made(json!({"words": "a person waves"})).1, "a_person_waves");
        assert_eq!(made(json!({"words": "a person waves", "name": "w"})).2, Kind::Move);
    }

    #[test]
    fn a_move_that_cannot_be_made_says_why() {
        let dir = std::env::temp_dir().join(format!("spatipal-moves-test-{}", std::process::id()));
        let (ask, name, kind) = made(json!({"words": "a person waves", "name": "wave"}));
        // Nothing listens there.
        let making = start("http://127.0.0.1:9", ask, name, kind, dir.clone());
        // Kept for the pal whose folder holds the shelf, and taken over nothing yet.
        assert_eq!((making.pal.as_path(), making.held_then.as_str(), making.forget, making.take), (std::env::temp_dir().as_path(), "", false, true));
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
        assert!(begun("twirl", Kind::Move).contains("appearance, move twirl") && begun("twirl", Kind::Move).contains("Not ready yet"));
        assert!(begun("one_leg", Kind::Pose).contains("appearance, pose one_leg"));
    }
}
