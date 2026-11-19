//! A person's motion made from words, through hemera (hemera's docs/media-api.md, section 10, and
//! docs/integrate-spatiand.md, section 5): a few seconds of whole-body motion on SMPL's 22 body
//! joints, which the room plays on a pal's own figure (`crates/spatiand/src/motion.rs`). Asked
//! for, waited for, fetched, and then dropped at hemera, which keeps nothing of it after that.
//!
//! The words are English and say how the body moves -- one or two actions, at most about sixty
//! words ("a person spins around once with the arms out"). A `seed` comes back with each motion:
//! with the same words it makes the same motion again. `loop` makes one that ends as it began,
//! for an idle.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::hemera::Hemera;

/// hemera's model for it.
pub const MODEL: &str = "hy-motion-1.0";

/// A motion as hemera gives it.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Motion {
    #[serde(default = "thirty")]
    pub fps: f32,
    #[serde(rename = "loop", default)]
    pub looping: bool,
    #[serde(default)]
    pub seed: u64,
    pub skeleton: Skeleton,
    pub frames: Vec<Frame>,
}

fn thirty() -> f32 {
    30.0
}

/// SMPL's body joints, by name, each with its parent (-1 for the root) and its place from its
/// parent at rest: a T-pose standing on y = 0, +Y up, facing +Z, the person's own left at +X.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Skeleton {
    pub joints: Vec<String>,
    pub parents: Vec<i32>,
    pub offsets: Vec<[f32; 3]>,
}

/// One moment: where the root (the pelvis) is in the world, and each joint's turn from its rest
/// (a unit quaternion, x y z w) -- the root's in the world, the others' from their parent's.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Frame {
    pub translation: [f32; 3],
    pub rotations: Vec<[f32; 4]>,
}

impl Motion {
    /// A joint's place in the lists, by its name (`L_Shoulder`).
    pub fn joint(&self, name: &str) -> Option<usize> {
        self.skeleton.joints.iter().position(|j| j == name)
    }

    /// How long it lasts: from its first frame to its last.
    pub fn duration(&self) -> f32 {
        self.frames.len().saturating_sub(1) as f32 / self.fps.max(1.0)
    }

    /// Whether it is one: its joints, parents, places and frames agreeing.
    pub fn check(&self) -> Result<(), String> {
        let n = self.skeleton.joints.len();
        if n == 0 || self.skeleton.parents.len() != n || self.skeleton.offsets.len() != n {
            return Err("the motion's skeleton does not add up".into());
        }
        if self.skeleton.parents.iter().enumerate().any(|(i, p)| *p >= i as i32 || (*p < 0 && i != 0)) {
            return Err("the motion's skeleton is not parents before children".into());
        }
        if self.frames.is_empty() || self.frames.iter().any(|f| f.rotations.len() != n) {
            return Err("the motion's frames do not fit its skeleton".into());
        }
        if self.fps.is_nan() || self.fps <= 0.0 {
            return Err("the motion has no frame rate".into());
        }
        Ok(())
    }
}

/// What is asked for.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Ask {
    /// How the body moves, in English.
    pub prompt: String,
    /// Seconds, 1 to 10.
    pub duration: f32,
    /// Ending as it began, for an idle.
    #[serde(rename = "loop", default)]
    pub looping: bool,
    /// The same words and seed make the same motion; none, and hemera draws one.
    #[serde(default)]
    pub seed: Option<u64>,
}

/// How long a motion is waited for, past hemera's loading its model (which its POST waits out).
const MAKING: Duration = Duration::from_secs(180);

/// A motion made: started, waited for (about 6 s, and 35 to 45 s more when hemera must load its
/// model first), fetched -- and then dropped at hemera, whether it was made or not. `progress`
/// hears how far it is, 0 to 100.
pub fn make(hemera: &Hemera, ask: &Ask, mut progress: impl FnMut(u32)) -> Result<Motion, String> {
    let prompt = ask.prompt.trim();
    if prompt.is_empty() {
        return Err("say how the body moves".into());
    }
    let mut request = json!({"model": MODEL, "prompt": prompt, "duration": ask.duration.clamp(1.0, 10.0), "fps": 30, "loop": ask.looping});
    if let Some(seed) = ask.seed {
        request["seed"] = json!(seed);
    }
    let job = hemera.post_json_waiting("/v1/motion/generations", &request, Duration::from_secs(120), 3)?;
    let id = job.get("id").and_then(Value::as_str).ok_or("hemera did not say which motion it was making")?.to_string();
    let path = format!("/v1/motion/generations/{id}");
    let made = wait(hemera, &path, &mut progress).and_then(|_| hemera.get_json(&format!("{path}/content")));
    let _ = hemera.delete(&path);
    let motion: Motion = serde_json::from_value(made?).map_err(|e| format!("hemera's motion was not readable: {e}"))?;
    motion.check()?;
    Ok(motion)
}

/// Until the motion is made, or why it was not.
fn wait(hemera: &Hemera, path: &str, progress: &mut impl FnMut(u32)) -> Result<(), String> {
    let since = Instant::now();
    loop {
        let job = hemera.get_json(path)?;
        match job.get("status").and_then(Value::as_str).unwrap_or_default() {
            "completed" => return Ok(()),
            "failed" | "cancelled" => {
                let why = job.pointer("/error/message").and_then(Value::as_str).unwrap_or("hemera could not make it");
                return Err(why.to_string());
            }
            _ => progress(job.get("progress").and_then(Value::as_u64).unwrap_or(0).min(100) as u32),
        }
        if since.elapsed() > MAKING {
            return Err("hemera took too long to make the motion".into());
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// A motion kept, to be played again without making it again: what it was made from -- the same
/// words and seed make it again exactly -- and the motion itself.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Kept {
    pub prompt: String,
    pub seed: u64,
    pub duration: f32,
    #[serde(rename = "loop", default)]
    pub looping: bool,
    pub motion: Motion,
}

/// A folder of kept motions -- a pal's `moves/` -- each as `<name>.json`, which the room reads
/// by its path (the avatar's `motions`); and BVH files the wearer put there (`crate::bvh`), each
/// by a name made of its file's (`Happy Dance.bvh`: `happy_dance`), which are theirs: never
/// written or let go of here. A motion made and kept by the same name is the one played.
pub struct Shelf {
    pub dir: std::path::PathBuf,
}

/// A name a motion can be kept by: small letters, digits and `_`, at most 32 of them.
pub fn name_of(words: &str) -> String {
    let mut name = String::new();
    for c in words.trim().to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            name.push(c);
        } else if !name.ends_with('_') && !name.is_empty() {
            name.push('_');
        }
        if name.len() >= 32 {
            break;
        }
    }
    name.trim_end_matches('_').to_string()
}

impl Shelf {
    /// The motions kept, by name, in order: those made and those given as BVH files.
    pub fn names(&self) -> Vec<String> {
        let mut all: Vec<String> = self.made().into_iter().chain(self.given().into_iter().map(|(n, _)| n)).collect();
        all.sort();
        all.dedup();
        all
    }

    /// Those made from words and kept here (`<name>.json`).
    pub fn made(&self) -> Vec<String> {
        let mut all: Vec<String> = std::fs::read_dir(&self.dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| e.file_name().to_str().and_then(|n| n.strip_suffix(".json")).map(str::to_string))
            .filter(|n| name_of(n) == *n && !n.is_empty())
            .collect();
        all.sort();
        all
    }

    /// The BVH files given (put here by the wearer), each by its name and its file.
    pub fn given(&self) -> Vec<(String, std::path::PathBuf)> {
        let mut all: Vec<(String, std::path::PathBuf)> = std::fs::read_dir(&self.dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("bvh")) && p.is_file())
            .filter_map(|p| Some((name_of(p.file_stem()?.to_str()?), p)))
            .filter(|(n, _)| !n.is_empty())
            .collect();
        all.sort();
        all.dedup_by(|a, b| a.0 == b.0);
        all
    }

    /// Where one made is kept, or would be.
    pub fn path(&self, name: &str) -> std::path::PathBuf {
        self.dir.join(format!("{}.json", name_of(name)))
    }

    /// The file played for a name: the one made, or else the BVH given.
    pub fn file(&self, name: &str) -> Option<std::path::PathBuf> {
        let made = self.path(name);
        if made.is_file() {
            return Some(made);
        }
        let name = name_of(name);
        self.given().into_iter().find(|(n, _)| *n == name).map(|(_, p)| p)
    }

    /// Whether the name is a BVH given, with no motion made by it.
    pub fn is_given(&self, name: &str) -> bool {
        !self.path(name).is_file() && self.file(name).is_some()
    }

    /// Keep a motion made, by `name`, over one of that name. Written whole or not at all, for its
    /// owner only (a pal's moves are its own).
    pub fn keep(&self, name: &str, ask: &Ask, motion: &Motion) -> Result<std::path::PathBuf, String> {
        let name = name_of(name);
        if name.is_empty() {
            return Err("a move needs a name to be kept by".into());
        }
        std::fs::create_dir_all(&self.dir).map_err(|e| format!("{}: {e}", self.dir.display()))?;
        let kept = Kept { prompt: ask.prompt.trim().to_string(), seed: motion.seed, duration: motion.duration(), looping: motion.looping, motion: motion.clone() };
        let text = serde_json::to_vec(&kept).map_err(|e| e.to_string())?;
        let path = self.path(&name);
        let partial = self.dir.join(format!(".{name}.json.partial"));
        std::fs::write(&partial, &text).map_err(|e| format!("{}: {e}", partial.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&partial, std::fs::Permissions::from_mode(0o600));
        }
        std::fs::rename(&partial, &path).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(path)
    }

    pub fn read(&self, name: &str) -> Result<Kept, String> {
        let path = self.path(name);
        let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        serde_json::from_slice(&bytes).map_err(|e| format!("{}: not a kept move ({e})", path.display()))
    }

    /// One made let go of; a BVH given never is.
    pub fn forget(&self, name: &str) -> Result<(), String> {
        let path = self.path(name);
        std::fs::remove_file(&path).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// What the room is told of them: each name to its file.
    pub fn room(&self) -> Value {
        Value::Object(self.names().into_iter().filter_map(|n| Some((n.clone(), json!(self.file(&n)?)))).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moves_are_kept_by_name_and_told_to_the_room() {
        assert_eq!(name_of("A slow Twirl!"), "a_slow_twirl");
        assert_eq!(name_of("  ---  "), "");
        assert_eq!(name_of(&"x".repeat(50)).len(), 32);
        let dir = std::env::temp_dir().join(format!("spatiand-motion-shelf-{}", std::process::id()));
        let shelf = Shelf { dir: dir.clone() };
        assert!(shelf.names().is_empty());
        let ask = Ask { prompt: "a person spins around once".into(), duration: 3.0, ..Default::default() };
        let path = shelf.keep("Twirl", &ask, &small()).unwrap();
        assert_eq!(path, dir.join("twirl.json"));
        assert_eq!(shelf.names(), ["twirl"]);
        let kept = shelf.read("twirl").unwrap();
        assert_eq!((kept.prompt.as_str(), kept.seed), ("a person spins around once", 7));
        assert_eq!(shelf.room()["twirl"], json!(path));
        assert!(shelf.keep("!!", &ask, &small()).is_err());
        shelf.forget("twirl").unwrap();
        assert!(shelf.names().is_empty());
        // BVH files the wearer put there: by names made of theirs, played by their own file.
        std::fs::write(dir.join("Happy Dance.BVH"), "HIERARCHY").unwrap();
        std::fs::write(dir.join("wave.bvh"), "HIERARCHY").unwrap();
        assert_eq!(shelf.names(), ["happy_dance", "wave"]);
        assert_eq!((shelf.made(), shelf.is_given("happy_dance")), (Vec::<String>::new(), true));
        assert_eq!(shelf.room()["happy_dance"], json!(dir.join("Happy Dance.BVH")));
        // Never let go of here: they are the wearer's.
        assert!(shelf.forget("wave").is_err());
        assert!(dir.join("wave.bvh").is_file());
        // One made by the same name is the one played.
        shelf.keep("wave", &ask, &small()).unwrap();
        assert_eq!((shelf.names(), shelf.is_given("wave")), (vec!["happy_dance".to_string(), "wave".to_string()], false));
        assert_eq!(shelf.room()["wave"], json!(dir.join("wave.json")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A motion of two frames on a chain of three joints.
    fn small() -> Motion {
        serde_json::from_value(json!({
            "object": "motion", "fps": 30, "loop": false, "seed": 7,
            "skeleton": {"joints": ["Pelvis", "Spine1", "Head"], "parents": [-1, 0, 1], "offsets": [[0, 0.97, 0], [0, 0.1, 0], [0, 0.5, 0]]},
            "frames": [
                {"translation": [0, 0.97, 0], "rotations": [[0, 0, 0, 1], [0, 0, 0, 1], [0, 0, 0, 1]]},
                {"translation": [0, 0.96, 0.01], "rotations": [[0, 0, 0, 1], [0.1, 0, 0, 0.995], [0, 0, 0, 1]]}
            ]
        }))
        .unwrap()
    }

    #[test]
    fn a_motion_is_read_and_checked() {
        let m = small();
        assert_eq!(m.joint("Spine1"), Some(1));
        assert!((m.duration() - 1.0 / 30.0).abs() < 1e-6);
        assert_eq!(m.seed, 7);
        m.check().unwrap();
        let mut bad = m.clone();
        bad.frames[1].rotations.pop();
        assert!(bad.check().is_err());
        let mut bad = m;
        bad.skeleton.parents = vec![-1, 2, 0];
        assert!(bad.check().is_err());
    }

    #[test]
    fn nothing_said_is_nothing_asked() {
        let hemera = Hemera::new("http://127.0.0.1:9");
        assert!(make(&hemera, &Ask { prompt: "  ".into(), duration: 3.0, ..Default::default() }, |_| {}).unwrap_err().contains("how the body moves"));
    }

    /// **Against hemera**: a wave made, fetched and dropped.
    /// `cargo test -p spatiand-voice motion -- --ignored`
    #[test]
    #[ignore]
    fn hemera_makes_a_wave() {
        let hemera = Hemera::new(&crate::settings::VoiceSettings::default().hemera);
        let ask = Ask { prompt: "a person waves with the right hand raised above the head".into(), duration: 2.0, looping: false, seed: Some(11) };
        let started = Instant::now();
        let mut heard = Vec::new();
        let m = make(&hemera, &ask, |p| heard.push(p)).unwrap();
        eprintln!("made in {:.1} s, {} frames, seed {}, progress {:?}", started.elapsed().as_secs_f32(), m.frames.len(), m.seed, heard);
        assert_eq!(m.skeleton.joints.len(), 22);
        assert_eq!(m.frames.len(), 60);
        assert_eq!(m.seed, 11);
        assert!(m.joint("R_Wrist").is_some());
    }
}
