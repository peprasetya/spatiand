//! A pal: SpatiPal as someone of the wearer's own making -- a name to answer to, who it is, a
//! figure that changes clothes and moves, a voice, models of its own -- kept in a folder of its
//! own beside SpatiPal's settings, and never in Spatiand's source (`docs/spatipal-pals.md`).
//!
//! ```text
//! ~/.config/spatiand/apps/assistant/pals/<folder>/
//!     pal.json     its name, its files, its voice and models; its figure's options, moves,
//!                  poses and faces in words, for its model
//!     persona.md   who it is: given to its model whole, every turn
//!     figure.glb   its figure (or a .vrm): options, animations, expressions
//!     look.json    how it looks now -- kept here as it changes, so it looks the same tomorrow
//! ```
//!
//! The folder is the shared one (`crate::store::shared`), so a pal is on every linked machine --
//! but a figure of more than 4 MB is not sent (spatiand-sync's limit for a setting). Its persona
//! and what is said to it go to the models on hemera and nowhere else; a pal marked `private`
//! has its words kept out of SpatiPal's log, and its chats are its own.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use spatiand_app::ai::AiSettings;
use spatiand_voice::figure::Offers;

use crate::store::Settings;

/// The states the room plays a figure's animations for by themselves: never offered as moves.
const STATES: &[&str] = &["idle", "listening", "checking", "attentive", "hearing", "transcribing", "thinking", "pondering", "working", "talking", "happy"];

/// Expressions the room moves by itself -- blinking, the mouth as it talks, the eyes' looks --
/// which a held face would fight.
const ROOM_FACES: &[&str] = &["blink", "blinkleft", "blinkright", "blink_l", "blink_r", "aa", "ih", "ou", "ee", "oh", "a", "i", "u", "e", "o", "lookup", "lookdown", "lookleft", "lookright", "neutral"];

/// The names of the tool's own choices: a group of options by one of these names is not offered.
const OWN: &[&str] = &["move", "pose", "own_pose", "face"];

/// How its model is told to make a pose of its own: the words the room reads
/// (`spatiand_voice::figure::aim`), and two poses looked at on a made-up figure -- none of them
/// one of [`STOCK`], which a small model then made again of its own, and half of it (neohorse-1:4b,
/// 2026-11-19: the upper arms of hands on hips without their forearms).
const OWN_POSE: &str = "Only for a pose that is not one of pose's: one of your own making, held until you choose another. \
Give each part you move and where it points, from your own point of view: front, back, up, down, left, right, out (away from your middle) or in (across it), \
together and with \"a little\". An upper arm points where its elbow goes, a forearm where its hand goes (or \"straight\"), a hand where its fingers go, \
a thigh where its knee goes, a shin where its foot goes (or \"straight\"). The head looks front, up, down, left or right, and can \"tilt left\" or \"tilt right\"; \
the chest leans front, back, left or right, and can \"turn left\" or \"turn right\"; a shoulder goes up, front or back. \
Leave out what stays as it is; an arm that moves needs its upper arm and its forearm. \
A hand on your heart: {\"right_upper_arm\": \"down, a little front, a little in\", \"right_forearm\": \"in, a little up\"}. \
Shy, looking down and away: {\"head\": \"down, right\", \"chest\": \"a little front\"}.";

/// Poses made of words, offered by name to a figure with a body beside its own animations: one
/// of the figure's of the same name is its own, and goes first; a part a figure lacks (its
/// shoulders) is left out. One whose name says `{side}` is offered for either side, as written
/// for the left and mirrored for the right. Each was looked at, from the front and the side, on
/// a made-up figure in the snapshot room (`tools/mannequin.py`; docs/spatipal-pals.md).
const STOCK: &[(&str, &str, Words)] = &[
    ("hands_on_hips", "both hands on your hips", &[("left_upper_arm", "down, out, a little back"), ("left_forearm", "in, a little down, a little front"), ("right_upper_arm", "down, out, a little back"), ("right_forearm", "in, a little down, a little front")]),
    (
        "{side}_hand_on_hip",
        "your {side} hand on your hip, leaning into it",
        &[("left_upper_arm", "down, out, a little back"), ("left_forearm", "in, a little down, a little front"), ("right_upper_arm", "down, a little out"), ("right_forearm", "down, a little front"), ("chest", "a little left"), ("head", "tilt right")],
    ),
    ("arms_folded", "your arms folded", &[("left_upper_arm", "down, a little front"), ("left_forearm", "in, a little up"), ("right_upper_arm", "down, a little front"), ("right_forearm", "in, a little up")]),
    (
        "thinking",
        "a hand to your chin, thinking",
        &[("right_upper_arm", "down, front"), ("right_forearm", "up, in, a little back"), ("left_upper_arm", "down, a little front"), ("left_forearm", "in, a little front"), ("head", "a little down, tilt right")],
    ),
    (
        "shrug",
        "a shrug",
        &[("left_shoulder", "up"), ("right_shoulder", "up"), ("left_upper_arm", "down, a little out"), ("right_upper_arm", "down, a little out"), ("left_forearm", "front, out"), ("right_forearm", "front, out"), ("head", "tilt left")],
    ),
    ("{side}_hand_raised", "your {side} hand up, as with a question", &[("left_upper_arm", "up, a little out"), ("left_forearm", "up"), ("right_upper_arm", "down, a little out"), ("right_forearm", "down, a little front")]),
    ("arms_up", "both arms up, cheering or stretching", &[("left_upper_arm", "up, a little out"), ("right_upper_arm", "up, a little out"), ("left_forearm", "straight"), ("right_forearm", "straight"), ("head", "a little up")]),
    (
        "hands_behind_head",
        "your hands behind your head, at ease",
        &[("left_upper_arm", "out, up"), ("right_upper_arm", "out, up"), ("left_forearm", "in, a little back"), ("right_forearm", "in, a little back"), ("left_hand", "in, back"), ("right_hand", "in, back")],
    ),
    (
        "lean_in",
        "leaning in, interested",
        &[("chest", "a little front"), ("head", "a little up, tilt left"), ("left_upper_arm", "down"), ("right_upper_arm", "down"), ("left_forearm", "down, a little front"), ("right_forearm", "down, a little front")],
    ),
    ("point", "pointing at the wearer", &[("right_upper_arm", "front"), ("right_forearm", "straight"), ("right_hand", "front"), ("left_upper_arm", "down, a little out"), ("left_forearm", "down, a little front")]),
    ("look_over_{side}_shoulder", "looking back over your {side} shoulder", &[("head", "left"), ("chest", "turn left"), ("left_upper_arm", "down"), ("right_upper_arm", "down")]),
];

/// Names, each with its words for the model (empty when `pal.json` gives none).
type Named = Vec<(String, String)>;

/// A pose made of words: each part, with where it points.
type Words = &'static [(&'static str, &'static str)];

/// Where pals are kept: on every linked machine.
pub fn root() -> PathBuf {
    crate::store::shared().join("assistant").join("pals")
}

/// The pals there are, by folder: each folder with a `pal.json`.
pub fn folders() -> Vec<String> {
    let mut all: Vec<String> = std::fs::read_dir(root())
        .map(|entries| entries.flatten().filter(|e| e.path().join("pal.json").is_file()).filter_map(|e| e.file_name().to_str().map(str::to_string)).collect())
        .unwrap_or_default();
    all.sort();
    all
}

/// What `pal.json` says. Every field may be left out.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct About {
    /// What it answers to and calls itself.
    pub name: String,
    /// Who it is: a file in its folder (`persona.md`), or, when there is no such file, the words.
    pub persona: String,
    /// Its figure, a file in its folder (`.glb`, `.vrm`).
    pub figure: String,
    /// How large its figure is against the room's usual (0.5 to 3).
    pub size: f32,
    /// A voice from hemera's library by name; or one described in words.
    pub voice: String,
    pub described: String,
    /// Its own everyday and larger models; the settings' when left out.
    pub model: String,
    pub large_model: String,
    /// What it hears and says is not written in SpatiPal's log, and its window describes no chat.
    pub private: bool,
    /// The robot's signs of what it is doing over its head (listening, thinking): shown unless
    /// `false`, for a figure whose animations show it.
    pub signs: Option<bool>,
    /// Its figure's options in words, by group, for its model: `{"outfit": {"dress": "a short
    /// black dress"}}`.
    pub options: BTreeMap<String, BTreeMap<String, String>>,
    /// Its animations that are played once (`wave`), and those held until asked otherwise
    /// (`hand_on_hip`), in words. One in neither is a move, if it is not one of the states'.
    pub moves: BTreeMap<String, String>,
    pub poses: BTreeMap<String, String>,
    /// Its expressions, in words: offered to hold (a pout, a smirk).
    pub faces: BTreeMap<String, String>,
    /// Its materials that may be given another colour, in words: `{"hair": "your hair"}`.
    pub colours: BTreeMap<String, String>,
}

/// How it looks now: what each group shows, the pose held -- one of its animations, or one of its
/// own making, by its parts' words -- the face held, colours asked for.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct Look {
    pub options: BTreeMap<String, String>,
    pub pose: String,
    pub own_pose: BTreeMap<String, String>,
    pub face: String,
    /// By material, `#rrggbb`.
    pub colours: BTreeMap<String, String>,
}

pub struct Pack {
    pub folder: String,
    pub dir: PathBuf,
    pub about: About,
    /// Who it is, as given to its model.
    pub persona: String,
    pub figure: Option<PathBuf>,
    /// What its figure offers, read from the file.
    pub offers: Offers,
    pub look: Look,
    /// Moves asked for: the room plays one each time this goes up.
    moves: u64,
    /// When its files last changed, as last read: read again when they change.
    stamps: Vec<Option<SystemTime>>,
}

impl Pack {
    /// The pal in `folder`.
    pub fn load(folder: &str) -> Result<Pack, String> {
        let dir = root().join(folder);
        let about_path = dir.join("pal.json");
        let text = std::fs::read_to_string(&about_path).map_err(|e| format!("{}: {e}", about_path.display()))?;
        let about: About = serde_json::from_str(&text).map_err(|e| format!("{} is not readable: {e}", about_path.display()))?;
        let persona = match about.persona.trim() {
            "" => String::new(),
            p => match std::fs::read_to_string(dir.join(p)) {
                Ok(words) => words,
                // No such file: the words are the persona -- unless they look like a file's name.
                Err(_) if p.ends_with(".md") || p.ends_with(".txt") => return Err(format!("{}: its persona {p} is not there", dir.display())),
                Err(_) => p.to_string(),
            },
        };
        let persona = without_comments(&persona);
        let figure = (!about.figure.trim().is_empty()).then(|| dir.join(about.figure.trim())).filter(|p| p.is_file());
        let offers = match &figure {
            Some(path) => spatiand_voice::figure::read(path).unwrap_or_else(|e| {
                log::warn!("pal {folder}: its figure says nothing of what it offers: {e}");
                Offers::default()
            }),
            None => Offers::default(),
        };
        let look = std::fs::read(dir.join("look.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let mut pack = Pack { folder: folder.to_string(), dir, about, persona, figure, offers, look, moves: 0, stamps: Vec::new() };
        pack.stamps = pack.stamps_now();
        Ok(pack)
    }

    /// A pal of no folder, for a test: its words and what its figure offers given.
    /// Its folder one of its own in the temporary folder, for what it keeps (`look.json`).
    #[cfg(test)]
    pub fn made_up(about: About, persona: &str, offers: Offers) -> Pack {
        static MADE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = MADE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("spatipal-made-up-{}-{n}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        Pack { folder: "made-up".into(), dir, about, persona: persona.to_string(), figure: None, offers, look: Look::default(), moves: 0, stamps: Vec::new() }
    }

    fn files(&self) -> Vec<PathBuf> {
        let mut files = vec![self.dir.join("pal.json")];
        if !self.about.persona.trim().is_empty() {
            files.push(self.dir.join(self.about.persona.trim()));
        }
        if !self.about.figure.trim().is_empty() {
            files.push(self.dir.join(self.about.figure.trim()));
        }
        files
    }

    fn stamps_now(&self) -> Vec<Option<SystemTime>> {
        self.files().iter().map(|f| std::fs::metadata(f).and_then(|m| m.modified()).ok()).collect()
    }

    /// Whether its files changed since they were read (edited, or brought from another machine).
    pub fn changed(&self) -> bool {
        self.stamps_now() != self.stamps
    }

    /// Taken as read as its files are now: they are not looked at again until they change again.
    pub fn mark_read(&mut self) {
        self.stamps = self.stamps_now();
    }

    /// The settings as this pal has them: its name, who it is, its voice and models over the
    /// wearer's.
    pub fn over(&self, s: &Settings) -> Settings {
        let mut s = s.clone();
        let a = &self.about;
        if !a.name.trim().is_empty() {
            s.name = a.name.trim().to_string();
        }
        if !self.persona.trim().is_empty() {
            s.persona = self.persona.trim().to_string();
        }
        if self.figure.is_some() {
            s.character = a.figure.trim().to_string();
        }
        if !a.voice.trim().is_empty() || !a.described.trim().is_empty() {
            s.voice = a.voice.trim().to_string();
            s.described = a.described.trim().to_string();
        }
        s
    }

    /// The settings to keep, from `edited` (as the window showed them, this pal's over the
    /// wearer's): what this pal sets goes back to the wearer's own, `saved`.
    pub fn under(&self, edited: &Settings, saved: &Settings) -> Settings {
        let mut s = edited.clone();
        let mine = self.over(saved);
        let theirs = |mine: &str, edited: &mut String, saved: &str| {
            if *edited == mine {
                *edited = saved.to_string();
            }
        };
        theirs(&mine.name, &mut s.name, &saved.name);
        theirs(&mine.persona, &mut s.persona, &saved.persona);
        theirs(&mine.character, &mut s.character, &saved.character);
        theirs(&mine.voice, &mut s.voice, &saved.voice);
        theirs(&mine.described, &mut s.described, &saved.described);
        s
    }

    /// The room's AI settings (`ai.json`, which SpatiPal reads and does not write) as this pal
    /// has them: its own everyday and larger models, when `pal.json` names them, over the room's.
    pub fn ai_over(&self, ai: &AiSettings) -> AiSettings {
        let mut ai = ai.clone();
        if !self.about.model.trim().is_empty() {
            ai.models.everyday = self.about.model.trim().to_string();
        }
        if !self.about.large_model.trim().is_empty() {
            ai.models.larger = self.about.large_model.trim().to_string();
        }
        ai
    }

    /// What it sets itself, of the settings' lines: `name`, `persona`, `figure`, `voice` (its
    /// models are over the room's AI settings, `ai_over`).
    pub fn sets(&self, what: &str) -> bool {
        let a = &self.about;
        match what {
            "name" => !a.name.trim().is_empty(),
            "persona" => !self.persona.trim().is_empty(),
            "figure" => self.figure.is_some(),
            "voice" => !a.voice.trim().is_empty() || !a.described.trim().is_empty(),
            _ => false,
        }
    }

    /// Its groups of options, as offered: each with its options (the figure's own order).
    fn groups(&self) -> impl Iterator<Item = &(String, Vec<String>)> {
        self.offers.groups.iter().filter(|(g, _)| !OWN.contains(&g.as_str()))
    }

    /// Its animations played once, and those held: each with its words, if `pal.json` has them.
    fn moves_and_poses(&self) -> (Named, Named) {
        let (mut moves, mut poses) = (Vec::new(), Vec::new());
        for name in &self.offers.animations {
            let bare = name.rsplit('|').next().unwrap_or(name);
            if STATES.iter().any(|s| s.eq_ignore_ascii_case(bare)) {
                continue;
            }
            let words = |map: &BTreeMap<String, String>| map.iter().find(|(k, _)| k.eq_ignore_ascii_case(bare)).map(|(_, v)| v.clone());
            match (words(&self.about.poses), words(&self.about.moves)) {
                (Some(w), _) => poses.push((bare.to_string(), w)),
                (None, w) => moves.push((bare.to_string(), w.unwrap_or_default())),
            }
        }
        (moves, poses)
    }

    /// The poses made of words it is offered by name ([`STOCK`]): none for a figure with no body,
    /// nor one it has an animation of the same name for; each with its words (`pal.json`'s, if it
    /// gives any) and only the parts its body has.
    fn stock(&self) -> Vec<(String, String, BTreeMap<String, String>)> {
        if self.offers.body.is_empty() {
            return Vec::new();
        }
        let own: Vec<&str> = self.offers.animations.iter().map(|a| a.rsplit('|').next().unwrap_or(a)).collect();
        let mut all: Vec<(String, String, Named)> = Vec::new();
        for (name, words, parts) in STOCK {
            let parts: Vec<(String, String)> = parts.iter().map(|(p, w)| (p.to_string(), w.to_string())).collect();
            if name.contains("{side}") {
                all.push((name.replace("{side}", "left"), words.replace("{side}", "left"), parts.clone()));
                all.push((name.replace("{side}", "right"), words.replace("{side}", "right"), parts.iter().map(|(p, w)| (mirrored(p), mirrored(w))).collect()));
            } else {
                all.push((name.to_string(), words.to_string(), parts));
            }
        }
        all.into_iter()
            .filter(|(name, _, _)| !own.iter().any(|a| a.eq_ignore_ascii_case(name)))
            .filter_map(|(name, words, parts)| {
                let parts: BTreeMap<String, String> = parts.into_iter().filter(|(p, _)| self.offers.body.contains(p)).collect();
                let words = self.about.poses.iter().find(|(k, _)| k.eq_ignore_ascii_case(&name)).map(|(_, w)| w.clone()).unwrap_or(words);
                (!parts.is_empty()).then_some((name, words, parts))
            })
            .collect()
    }

    /// Its expressions that may be held: not the room's own (blinking, the mouth, the looks).
    fn faces(&self) -> Vec<(String, String)> {
        self.offers
            .expressions
            .iter()
            .filter(|e| !ROOM_FACES.contains(&e.to_lowercase().as_str()))
            .map(|e| (e.clone(), self.about.faces.iter().find(|(k, _)| k.eq_ignore_ascii_case(e)).map(|(_, v)| v.clone()).unwrap_or_default()))
            .collect()
    }

    /// Its materials that may be given another colour: those `pal.json` names that the figure has.
    fn colourable(&self) -> Vec<(String, String)> {
        self.about.colours.iter().filter(|(m, _)| self.offers.materials.contains(&m.to_lowercase())).map(|(m, w)| (m.to_lowercase(), w.clone())).collect()
    }

    /// The tool its models are given to change how it looks and move: `None` when its figure
    /// offers nothing.
    pub fn tool(&self) -> Option<Value> {
        let listed = |all: &[(String, String)]| all.iter().map(|(n, w)| if w.is_empty() { n.clone() } else { format!("{n} ({w})") }).collect::<Vec<_>>().join(", ");
        let mut properties = serde_json::Map::new();
        let mut said = Vec::new();
        for (group, options) in self.groups() {
            let words: Vec<(String, String)> = options
                .iter()
                .map(|o| (o.clone(), self.about.options.iter().find(|(g, _)| g.eq_ignore_ascii_case(group)).and_then(|(_, m)| m.iter().find(|(k, _)| k.eq_ignore_ascii_case(o))).map(|(_, w)| w.clone()).unwrap_or_default()))
                .collect();
            properties.insert(group.clone(), json!({"type": "string", "enum": options, "description": format!("Your {group}: {}", listed(&words))}));
            said.push(group.clone());
        }
        let (moves, poses) = self.moves_and_poses();
        if !moves.is_empty() {
            properties.insert("move".into(), json!({"type": "string", "enum": moves.iter().map(|(n, _)| n).collect::<Vec<_>>(), "description": format!("Something to do once: {}", listed(&moves))}));
            said.push("a move".into());
        }
        let body = &self.offers.body;
        // Its own poses, then those made of words for a figure with a body.
        let poses: Named = poses.into_iter().chain(self.stock().into_iter().map(|(n, w, _)| (n, w))).collect();
        if !poses.is_empty() || !body.is_empty() {
            let mut names: Vec<&String> = poses.iter().map(|(n, _)| n).collect();
            let none = "none".to_string();
            names.push(&none);
            let description = match (poses.is_empty(), body.is_empty()) {
                (false, true) => format!("A pose to hold until you choose another, or none: {}", listed(&poses)),
                (false, false) => format!("A pose to hold until you choose another, or none (or make your own with own_pose): {}", listed(&poses)),
                _ => "none: no pose held (make one with own_pose)".to_string(),
            };
            properties.insert("pose".into(), json!({"type": "string", "enum": names, "description": description}));
            said.push("a pose".into());
        }
        if !body.is_empty() {
            // Its parts named in words, not as the object's properties: a model's call is held to
            // its schema, and hemera's (llama.cpp's) holds an object's properties to the order
            // they are listed in -- alphabetical, here -- so that a forearm could not follow its
            // upper arm, and a pose ended at the right upper arm (2026-11-19, both models).
            let named: Vec<&str> = poses.iter().map(|(n, _)| n.as_str()).collect();
            let mut description = format!("{OWN_POSE} Your parts: {}.", body.join(", "));
            if !named.is_empty() {
                description.push_str(&format!(" Not for {}: choose those in pose.", named.join(", ")));
            }
            properties.insert("own_pose".into(), json!({"type": "object", "description": description, "additionalProperties": {"type": "string"}}));
            said.push("a pose of your own".into());
        }
        let faces = self.faces();
        if !faces.is_empty() {
            let mut names: Vec<&String> = faces.iter().map(|(n, _)| n).collect();
            let none = "none".to_string();
            names.push(&none);
            properties.insert("face".into(), json!({"type": "string", "enum": names, "description": format!("An expression to hold, or none: {}", listed(&faces))}));
            said.push("a face".into());
        }
        for (material, words) in self.colourable() {
            properties.insert(format!("{material}_colour"), json!({"type": "string", "description": format!("A new colour for {}, as #rrggbb or a colour's name", if words.is_empty() { material.clone() } else { words })}));
        }
        if properties.is_empty() {
            return None;
        }
        let description = format!(
            "Change how you look in the room, or do something with your body: {}. Give only what changes. Do it when the wearer asks, or when you feel like it; then say it, briefly, in your own way.",
            said.join(", ")
        );
        Some(spatiand_voice::chat::tool("appearance", &description, Value::Object(properties), &[]))
    }

    /// A call of its tool: the look changed and kept, and what the room is to be told -- or why
    /// not, in words its model can mend its call by.
    pub fn apply(&mut self, args: &Value) -> Result<(Value, String), String> {
        let mut did = Vec::new();
        let mut look = self.look.clone();
        let mut moved = None;
        let groups: Vec<(String, Vec<String>)> = self.groups().cloned().collect();
        for (key, value) in args.as_object().into_iter().flatten() {
            if key.eq_ignore_ascii_case("own_pose") {
                let made = self.own_pose(value)?;
                did.push(format!("a pose of your own: {}", made.keys().cloned().collect::<Vec<_>>().join(", ")));
                look.own_pose = made;
                look.pose.clear();
                continue;
            }
            let Some(value) = value.as_str().map(str::trim).filter(|v| !v.is_empty()) else { continue };
            let wanted = value.to_lowercase();
            let key = key.to_lowercase();
            if let Some((group, options)) = groups.iter().find(|(g, _)| *g == key) {
                if !options.contains(&wanted) {
                    return Err(format!("{group} has no {value}: choose one of {}", options.join(", ")));
                }
                look.options.insert(group.clone(), wanted.clone());
                did.push(format!("{group}: {wanted}"));
                continue;
            }
            let (moves, poses) = self.moves_and_poses();
            let named = |all: &[(String, String)]| all.iter().find(|(n, _)| n.eq_ignore_ascii_case(value)).map(|(n, _)| n.clone());
            match key.as_str() {
                "move" => match named(&moves) {
                    Some(m) => {
                        did.push(m.clone());
                        moved = Some(m);
                    }
                    None => return Err(format!("there is no move {value}: choose one of {}", moves.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(", "))),
                },
                "pose" if wanted == "none" => {
                    look.pose.clear();
                    look.own_pose.clear();
                    did.push("no pose".into());
                }
                "pose" => match (named(&poses), self.stock().into_iter().find(|(n, _, _)| n.eq_ignore_ascii_case(value))) {
                    (Some(p), _) => {
                        did.push(format!("pose: {p}"));
                        look.pose = p;
                        look.own_pose.clear();
                    }
                    // One made of words: held as its words, by its name.
                    (None, Some((name, _, parts))) => {
                        did.push(format!("pose: {name}"));
                        look.pose = name;
                        look.own_pose = parts;
                    }
                    (None, None) => {
                        let all: Vec<String> = poses.iter().map(|(n, _)| n.clone()).chain(self.stock().into_iter().map(|(n, _, _)| n)).collect();
                        return Err(format!("there is no pose {value}: choose one of {}, or none{}", all.join(", "), if self.offers.body.is_empty() { "" } else { " -- or make your own with own_pose" }));
                    }
                },
                "face" if wanted == "none" => {
                    look.face.clear();
                    did.push("face: as it was".into());
                }
                "face" => match self.faces().into_iter().find(|(n, _)| n.eq_ignore_ascii_case(value)) {
                    Some((f, _)) => {
                        did.push(format!("face: {f}"));
                        look.face = f;
                    }
                    None => return Err(format!("there is no face {value}: choose one of {}, or none", self.faces().iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(", "))),
                },
                _ => match key.strip_suffix("_colour").or_else(|| key.strip_suffix("_color")).filter(|m| self.colourable().iter().any(|(c, _)| c == m)) {
                    Some(material) => match colour(value) {
                        Some(hex) => {
                            did.push(format!("{material}: {value}"));
                            look.colours.insert(material.to_string(), hex);
                        }
                        None => return Err(format!("{value} is not a colour to me: give it as #rrggbb")),
                    },
                    None => return Err(format!("you cannot change {key}")),
                },
            }
        }
        if did.is_empty() {
            return Err("nothing was chosen: give what to change".into());
        }
        if look != self.look {
            self.look = look;
            self.keep_look();
        }
        let mut room = self.figure_request(true);
        if let Some(m) = moved {
            self.moves += 1;
            room["move"] = json!(m);
            room["moves"] = json!(self.moves);
        }
        // Only what it says of itself: shown or not is the settings' (`tell_figure`).
        room.as_object_mut().map(|r| r.remove("visible"));
        Ok((room, did.join(", ")))
    }

    /// A pose of its own making, from its tool's call: each part it names one its figure has,
    /// in words the room reads -- or why not, so that its model can mend them. A part left as it
    /// moves ("relaxed") is left out.
    pub(crate) fn own_pose(&self, value: &Value) -> Result<BTreeMap<String, String>, String> {
        let parts = match value {
            Value::Object(m) => m.clone(),
            // A small model may write the object as a string of JSON.
            Value::String(text) => serde_json::from_str::<Value>(text).ok().and_then(|v| v.as_object().cloned()).ok_or("own_pose: give the parts you move, each with where it points")?,
            _ => return Err("own_pose: give the parts you move, each with where it points".into()),
        };
        let mut made = BTreeMap::new();
        for (part, words) in &parts {
            let part = part.trim().to_lowercase();
            if !self.offers.body.contains(&part) {
                return Err(format!("you have no {part}: your parts are {}", self.offers.body.join(", ")));
            }
            let Some(words) = words.as_str() else { return Err(format!("{part}: say in words where it points")) };
            if spatiand_voice::figure::aim(&part, words)?.is_some() {
                made.insert(part, words.trim().to_string());
            }
        }
        if made.is_empty() {
            return Err("own_pose: name the parts you move, each with where it points".into());
        }
        Ok(made)
    }

    fn keep_look(&self) {
        match serde_json::to_string_pretty(&self.look) {
            Ok(text) => {
                if let Err(e) = crate::store::write(&self.dir.join("look.json"), &text) {
                    log::warn!("pal {}: how it looks could not be kept: {e}", self.folder);
                }
            }
            Err(e) => log::warn!("pal {}: {e}", self.folder),
        }
    }

    /// What the room is told of its figure: which file, how large, how it looks now.
    pub fn figure_request(&self, shown: bool) -> Value {
        let options: BTreeMap<&String, &String> = self.look.options.iter().filter(|(g, o)| self.offers.groups.iter().any(|(group, all)| group == *g && all.contains(o))).collect();
        let mut room = json!({
            "do": "avatar",
            "character": format!("pal:{}", self.folder),
            "path": self.figure,
            "visible": shown,
            "options": options,
            "pose": if self.look.own_pose.is_empty() { json!(self.look.pose) } else { json!(self.look.own_pose) },
            "expression": self.look.face,
            "colors": self.look.colours,
        });
        if self.about.size > 0.0 {
            room["size"] = json!(self.about.size);
        }
        if let Some(signs) = self.about.signs {
            room["signs"] = json!(signs);
        }
        room
    }

    /// How it looks now, in words, for its model with the wearer's words: what each group shows,
    /// the pose and the face held. Empty for a figure that offers nothing.
    pub fn note(&self) -> String {
        let mut parts = Vec::new();
        for (group, options) in self.groups() {
            let now = self.look.options.get(group).filter(|o| options.contains(o)).or(options.first());
            if let Some(o) = now {
                parts.push(format!("{group} {o}"));
            }
        }
        if !self.look.pose.is_empty() {
            parts.push(format!("holding the pose {}", self.look.pose));
        }
        if self.look.pose.is_empty() && !self.look.own_pose.is_empty() {
            let held: Vec<String> = self.look.own_pose.iter().map(|(part, words)| format!("{part} \"{words}\"")).collect();
            parts.push(format!("holding a pose of your own: {}", held.join(", ")));
        }
        if !self.look.face.is_empty() {
            parts.push(format!("your face {}", self.look.face));
        }
        for (material, hex) in &self.look.colours {
            parts.push(format!("{material} {hex}"));
        }
        if parts.is_empty() {
            return String::new();
        }
        format!("[How you look now: {}.]", parts.join("; "))
    }
}

/// Left as right and right as left: a part's name, or a pose's words.
fn mirrored(text: &str) -> String {
    text.replace("left", "\u{1}").replace("right", "left").replace('\u{1}', "right")
}

/// A persona as its model is given it: without the HTML comments in it (`<!-- ... -->`), which
/// are its maker's notes to themself.
fn without_comments(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        match rest[start..].find("-->") {
            Some(end) => rest = &rest[start + end + 3..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    // Blank lines left where a comment was are closed up.
    let mut tidy = String::new();
    let mut blank = 0;
    for line in out.lines() {
        if line.trim().is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
        } else {
            blank = 0;
        }
        tidy.push_str(line);
        tidy.push('\n');
    }
    tidy.trim().to_string()
}

/// A colour as a model may give it: `#rrggbb`, `rrggbb`, or a common name.
fn colour(text: &str) -> Option<String> {
    let t = text.trim().to_lowercase();
    let hex = t.trim_start_matches('#');
    if hex.len() == 6 && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Some(format!("#{hex}"));
    }
    let named = match t.as_str() {
        "black" | "jet black" => "#141414",
        "white" => "#f2f2f2",
        "silver" | "grey" | "gray" => "#a8a8b0",
        "platinum" | "platinum blonde" => "#e6dcc8",
        "blonde" | "blond" => "#d8b870",
        "golden" | "gold" => "#d4a030",
        "brown" => "#5a3a22",
        "dark brown" => "#3a2414",
        "chestnut" => "#6e3c1e",
        "auburn" => "#7a2e1a",
        "ginger" | "copper" => "#b5562a",
        "red" => "#c01c24",
        "dark red" | "burgundy" | "wine" => "#6a1020",
        "pink" => "#e87aa8",
        "hot pink" => "#e0287a",
        "purple" => "#6a2a9a",
        "violet" | "lavender" => "#9a7ad8",
        "blue" => "#2a52c8",
        "navy" => "#1a2450",
        "teal" => "#1a8a8a",
        "green" => "#2a8a3a",
        "orange" => "#e07020",
        "yellow" => "#e8c830",
        _ => return None,
    };
    Some(named.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pack(about: About, offers: Offers) -> Pack {
        // A folder of each test's own: they run side by side.
        static MADE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = MADE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("spatipal-pack-test-{}-{n}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        Pack {
            folder: "test".into(),
            dir,
            about,
            persona: "You are someone.".into(),
            figure: None,
            offers,
            look: Look::default(),
            moves: 0,
            stamps: Vec::new(),
        }
    }

    fn offers() -> Offers {
        Offers {
            groups: vec![("outfit".into(), vec!["casual".into(), "dress".into()]), ("hair".into(), vec!["long".into(), "bob".into()])],
            animations: vec!["idle".into(), "talking".into(), "Armature|wave".into(), "hand_on_hip".into()],
            expressions: vec!["blink".into(), "aa".into(), "smirk".into(), "pout".into()],
            materials: vec!["hair".into(), "skin".into()],
            body: Vec::new(),
        }
    }

    fn about() -> About {
        let mut a = About { name: "Mira".into(), ..Default::default() };
        a.poses.insert("hand_on_hip".into(), "a hand on your hip".into());
        a.moves.insert("wave".into(), "a wave".into());
        a.options.insert("outfit".into(), [("dress".to_string(), "a short black dress".to_string())].into_iter().collect());
        a.colours.insert("hair".into(), "your hair".into());
        a
    }

    #[test]
    fn the_tool_offers_what_the_figure_has_and_not_what_the_room_does_by_itself() {
        let p = pack(about(), offers());
        let tool = p.tool().unwrap();
        let props = &tool["function"]["parameters"]["properties"];
        assert_eq!(props["outfit"]["enum"], json!(["casual", "dress"]));
        assert!(props["outfit"]["description"].as_str().unwrap().contains("dress (a short black dress)"));
        // The states' animations are the room's; `wave` is a move, `hand_on_hip` a pose.
        assert_eq!(props["move"]["enum"], json!(["wave"]));
        assert_eq!(props["pose"]["enum"], json!(["hand_on_hip", "none"]));
        // Blinking and the mouth are the room's.
        assert_eq!(props["face"]["enum"], json!(["smirk", "pout", "none"]));
        assert!(props.get("hair_colour").is_some());
        assert!(props.get("skin_colour").is_none());
        // A figure that offers nothing has no tool.
        assert!(pack(About::default(), Offers::default()).tool().is_none());
    }

    #[test]
    fn a_call_changes_the_look_and_tells_the_room() {
        let mut p = pack(about(), offers());
        let (room, did) = p.apply(&json!({"outfit": "Dress", "move": "wave", "face": "smirk"})).unwrap();
        assert_eq!(room["options"], json!({"outfit": "dress"}));
        assert_eq!(room["move"], "wave");
        assert_eq!(room["moves"], 1);
        assert_eq!(room["expression"], "smirk");
        assert!(room.get("visible").is_none());
        assert!(did.contains("outfit: dress"));
        // The same move again is played again.
        let (room, _) = p.apply(&json!({"move": "wave"})).unwrap();
        assert_eq!(room["moves"], 2);
        assert_eq!(p.note(), "[How you look now: outfit dress; hair long; your face smirk.]");
        let _ = std::fs::remove_dir_all(&p.dir);
    }

    #[test]
    fn a_call_for_what_is_not_there_says_what_is() {
        let mut p = pack(about(), offers());
        let why = p.apply(&json!({"outfit": "bikini"})).unwrap_err();
        assert!(why.contains("casual, dress"), "{why}");
        assert!(p.apply(&json!({"pose": "dance"})).is_err());
        assert!(p.apply(&json!({"skin_colour": "#000000"})).is_err());
        assert!(p.apply(&json!({})).is_err());
        assert_eq!(p.look, Look::default());
    }

    #[test]
    fn a_pose_of_its_own_is_offered_checked_kept_and_told() {
        let mut offers = offers();
        offers.body = spatiand_voice::figure::BODY_PARTS.iter().filter(|p| !p.ends_with("_shoulder")).map(|p| p.to_string()).collect();
        let mut p = pack(about(), offers);
        let tool = p.tool().unwrap();
        let props = &tool["function"]["parameters"]["properties"];
        assert_eq!(props["own_pose"]["type"], "object");
        // Its parts in words, in any order: not properties a call is held to the order of.
        assert!(props["own_pose"].get("properties").is_none());
        let words = props["own_pose"]["description"].as_str().unwrap();
        let parts = words.split("Your parts: ").nth(1).and_then(|w| w.split('.').next()).unwrap();
        assert!(parts.contains("left_forearm") && !parts.contains("left_shoulder"), "{parts}");
        // Its words as the room reads them, a part left relaxed left out.
        let (room, did) = p.apply(&json!({"pose": "hand_on_hip"})).unwrap();
        assert_eq!(room["pose"], "hand_on_hip");
        assert!(did.contains("hand_on_hip"));
        let (room, did) = p.apply(&json!({"own_pose": {"left_upper_arm": "down, out, a little back", "Left_Forearm": "in, a little front", "head": "relaxed"}})).unwrap();
        assert_eq!(room["pose"], json!({"left_forearm": "in, a little front", "left_upper_arm": "down, out, a little back"}));
        assert!(did.contains("a pose of your own"), "{did}");
        assert_eq!(p.look.pose, "");
        assert!(p.note().contains("holding a pose of your own: left_forearm \"in, a little front\""), "{}", p.note());
        // Written as a string of JSON, as a small model may.
        let (room, _) = p.apply(&json!({"own_pose": "{\"head\": \"a little down\"}"})).unwrap();
        assert_eq!(room["pose"], json!({"head": "a little down"}));
        // What it has not, and words that say nothing, are answered with what would do.
        assert!(p.apply(&json!({"own_pose": {"left_shoulder": "up"}})).unwrap_err().contains("your parts are"));
        assert!(p.apply(&json!({"own_pose": {"left_hand": "on my hip"}})).unwrap_err().contains("front, back, up"));
        assert_eq!(p.look.own_pose.get("head").map(String::as_str), Some("a little down"));
        // One made of words, by its name: held as its words; one the figure has its own of is
        // its own; and one using a part it has not (shoulders) without it.
        let tool = p.tool().unwrap();
        let names = tool["function"]["parameters"]["properties"]["pose"]["enum"].clone();
        assert!(names.as_array().unwrap().iter().any(|n| n == "hands_on_hips"), "{names}");
        assert_eq!(names.as_array().unwrap().iter().filter(|n| *n == "hand_on_hip").count(), 1);
        // Either side of one that has a side, the right mirrored from the left.
        let (room, _) = p.apply(&json!({"pose": "look_over_right_shoulder"})).unwrap();
        assert_eq!((room["pose"]["head"].as_str(), room["pose"]["chest"].as_str()), (Some("right"), Some("turn right")));
        let (room, _) = p.apply(&json!({"pose": "right_hand_on_hip"})).unwrap();
        assert_eq!(room["pose"]["right_forearm"], "in, a little down, a little front");
        assert_eq!(room["pose"]["head"], "tilt left");
        let (room, _) = p.apply(&json!({"pose": "Hands_On_Hips"})).unwrap();
        assert_eq!(room["pose"]["left_forearm"], "in, a little down, a little front");
        assert_eq!(p.note(), "[How you look now: outfit casual; hair long; holding the pose hands_on_hips.]");
        let (room, _) = p.apply(&json!({"pose": "shrug"})).unwrap();
        assert!(room["pose"].get("left_shoulder").is_none() && room["pose"].get("left_forearm").is_some());
        let (room, _) = p.apply(&json!({"pose": "hand_on_hip"})).unwrap();
        assert_eq!(room["pose"], "hand_on_hip");
        p.apply(&json!({"own_pose": {"head": "a little down"}})).unwrap();
        // None lets go of it; it was kept in look.json.
        let kept: Look = serde_json::from_str(&std::fs::read_to_string(p.dir.join("look.json")).unwrap()).unwrap();
        assert_eq!(kept.own_pose.len(), 1);
        p.apply(&json!({"pose": "none"})).unwrap();
        assert!(p.look.own_pose.is_empty());
        let _ = std::fs::remove_dir_all(&p.dir);
    }

    #[test]
    fn colours_by_name_or_number() {
        assert_eq!(colour("#AA00ff").as_deref(), Some("#aa00ff"));
        assert_eq!(colour("Platinum Blonde").as_deref(), Some("#e6dcc8"));
        assert_eq!(colour("the colour of the sea"), None);
        let mut p = pack(about(), offers());
        p.apply(&json!({"hair_colour": "red"})).unwrap();
        assert_eq!(p.look.colours.get("hair").map(String::as_str), Some("#c01c24"));
        let _ = std::fs::remove_dir_all(&p.dir);
    }

    #[test]
    fn its_settings_go_over_the_wearers_and_back() {
        let mut a = about();
        a.model = "own-model".into();
        let p = pack(a, Offers::default());
        let saved = Settings::default();
        let shown = p.over(&saved);
        assert_eq!(shown.name, "Mira");
        assert_eq!(shown.persona, "You are someone.");
        // Its models over the room's AI settings: its own everyday one, the room's larger.
        let room = AiSettings::default();
        let ai = p.ai_over(&room);
        assert_eq!(ai.models.everyday, "own-model");
        assert_eq!(ai.models.larger, room.models.larger);
        // A setting changed in the window is kept; what the pal set goes back to the wearer's.
        let mut edited = shown.clone();
        edited.spoken = !saved.spoken;
        let kept = p.under(&edited, &saved);
        assert_eq!(kept.name, saved.name);
        assert_eq!(kept.persona, saved.persona);
        assert_eq!(kept.spoken, !saved.spoken);
    }

    #[test]
    fn a_personas_comments_are_its_makers_own() {
        let text = "<!-- notes to myself -->\nYou are Mira.\n\n<!-- more\nnotes -->\n\nYou tease.";
        assert_eq!(without_comments(text), "You are Mira.\n\nYou tease.");
    }
}
