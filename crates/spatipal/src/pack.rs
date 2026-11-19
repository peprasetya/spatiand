//! A pal: who SpatiPal is -- a name to answer to, a personality, a figure that changes clothes
//! and moves, a voice, models of its own (`docs/spatipal-pals.md`). **Pixel is the built-in one**,
//! first among them: its name, personality, figure and voice are SpatiPal's own settings
//! (`assistant.json`), as they always were. Every other is a folder of the wearer's own making
//! beside them, never in Spatiand's source:
//!
//! ```text
//! ~/.config/spatiand/apps/assistant/pals/<folder>/
//!     pal.json     its name, its files, its voice and models; its figure's options, moves,
//!                  poses and faces in words, for its model
//!     persona.md   who it is: given to its model whole, every turn
//!     figure.glb   its figure (or a .vrm): options, animations, expressions
//!     look.json    how it looks now -- kept here as it changes, so it looks the same tomorrow
//!     moves/       moves made for it, `<name>.json` each
//! ```
//!
//! Pixel keeps what changes as it is used -- its look, the moves made for it -- in `pals/pixel/`,
//! made the first time something is kept there; it has no `pal.json` (its definition is the
//! settings'), and one put there is not read: the folder is Pixel's.
//!
//! The folder is the shared one (`crate::store::shared`), so a pal is on every linked machine --
//! but a figure of more than 4 MB is not sent (spatiand-sync's limit for a setting). Its persona
//! and what is said to it go to the models on hemera and nowhere else; a pal marked `private`
//! has its words kept out of SpatiPal's log, and its chats are its own (Pixel's are those had
//! with no pal before there were pals).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
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
        "hands_behind_back",
        "your hands clasped behind your back",
        &[("left_upper_arm", "down, a little back, a little out"), ("right_upper_arm", "down, a little back, a little out"), ("left_forearm", "in, a little back, a little down"), ("right_forearm", "in, a little back, a little down"), ("chest", "a little back")],
    ),
    (
        "lean_in",
        "leaning in, interested",
        &[("chest", "a little front"), ("head", "a little up, tilt left"), ("left_upper_arm", "down"), ("right_upper_arm", "down"), ("left_forearm", "down, a little front"), ("right_forearm", "down, a little front")],
    ),
    ("point", "pointing at the wearer", &[("right_upper_arm", "front"), ("right_forearm", "straight"), ("right_hand", "front"), ("left_upper_arm", "down, a little out"), ("left_forearm", "down, a little front")]),
    ("look_over_{side}_shoulder", "looking back over your {side} shoulder", &[("head", "left"), ("chest", "turn left"), ("left_upper_arm", "down"), ("right_upper_arm", "down")]),
    // The heart is on her left: a right hand crosses to it, a left one rises in front of its own side.
    (
        "right_hand_on_heart",
        "your right hand on your heart",
        &[("right_upper_arm", "down, a little front, a little in"), ("right_forearm", "in, a little up"), ("left_upper_arm", "down, a little out"), ("left_forearm", "down, a little front")],
    ),
    (
        "left_hand_on_heart",
        "your left hand on your heart",
        &[("left_upper_arm", "down, a little front"), ("left_forearm", "up, in"), ("right_upper_arm", "down, a little out"), ("right_forearm", "down, a little front")],
    ),
    (
        "look_up",
        "looking up",
        &[("head", "up"), ("chest", "a little back"), ("left_upper_arm", "down, a little out"), ("right_upper_arm", "down, a little out"), ("left_forearm", "down, a little front"), ("right_forearm", "down, a little front")],
    ),
    (
        "look_down",
        "looking down, shy",
        &[("head", "down"), ("chest", "a little front"), ("left_upper_arm", "down, a little out"), ("right_upper_arm", "down, a little out"), ("left_forearm", "down, a little front"), ("right_forearm", "down, a little front")],
    ),
    (
        "head_tilt",
        "your head tilted, curious",
        &[("head", "tilt left, a little down"), ("left_upper_arm", "down, a little out"), ("right_upper_arm", "down, a little out"), ("left_forearm", "down, a little front"), ("right_forearm", "down, a little front")],
    ),
];

/// Names, each with its words for the model (empty when `pal.json` gives none).
type Named = Vec<(String, String)>;

/// A pose made of words: each part, with where it points.
type Words = &'static [(&'static str, &'static str)];

/// Where pals are kept: on every linked machine.
pub fn root() -> PathBuf {
    crate::store::shared().join("assistant").join("pals")
}

/// Pixel's own folder in [`root`]: its look and its moves, never a pal of that name.
pub const PIXEL: &str = "pixel";

/// The pals installed, by folder: each folder with a `pal.json` -- Pixel's own excepted. Pixel,
/// built in, is not among them: it is always there, before them.
pub fn folders() -> Vec<String> {
    folders_in(&root())
}

fn folders_in(root: &Path) -> Vec<String> {
    let mut all: Vec<String> = std::fs::read_dir(root)
        .map(|entries| entries.flatten().filter(|e| e.path().join("pal.json").is_file()).filter_map(|e| e.file_name().to_str().map(str::to_string)).collect())
        .unwrap_or_default();
    all.retain(|f| f != PIXEL);
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

impl Look {
    /// The pose it holds, by name or by its parts' words, as one string to tell one from another.
    pub fn holding(&self) -> String {
        format!("{}{:?}", self.pose, self.own_pose)
    }
}

/// A pal: Pixel, built in (its `folder` empty), or one installed in a folder of its own.
pub struct Pack {
    /// Its folder in [`root`], by which it is chosen (`Settings::pal`); empty for Pixel.
    pub folder: String,
    /// Its own folder: what it keeps of itself is there (Pixel's is [`PIXEL`]).
    pub dir: PathBuf,
    /// What `pal.json` says -- for Pixel, what its settings say, in the same terms.
    pub about: About,
    /// Who it is, as given to its model.
    pub persona: String,
    pub figure: Option<PathBuf>,
    /// What its figure offers, read from the file.
    pub offers: Offers,
    pub look: Look,
    /// Its files can be changed from its settings: an installed pal's folder, `pal.json` and
    /// persona are the wearer's to write here (Pixel's are SpatiPal's settings, always).
    pub writable: bool,
    /// Moves asked for: the room plays one each time this goes up.
    moves: u64,
    /// When its files last changed, as last read: read again when they change.
    stamps: Vec<Option<SystemTime>>,
}

impl Pack {
    /// Pixel, built in: its name, personality, figure and voice as SpatiPal's settings have them.
    pub fn pixel(s: &Settings) -> Pack {
        Pack::pixel_at(&root(), s)
    }

    /// Pixel, with `root` for where pals are kept.
    pub fn pixel_at(root: &Path, s: &Settings) -> Pack {
        let figure = crate::store::character_path(&s.character);
        let about = About { name: s.name.clone(), figure: s.character.clone(), voice: s.voice.clone(), described: s.described.clone(), ..Default::default() };
        let dir = root.join(PIXEL);
        let mut pack = Pack { folder: String::new(), offers: offers_of(figure.as_deref(), "pixel"), look: look_in(&dir), dir, about, persona: s.persona.clone(), figure, writable: true, moves: 0, stamps: Vec::new() };
        pack.stamps = pack.stamps_now();
        pack
    }

    /// Whether it is Pixel, the built-in pal.
    pub fn is_pixel(&self) -> bool {
        self.folder.is_empty()
    }

    /// What it answers to and calls itself: an installed pal with no name in its `pal.json` is
    /// called by its folder's.
    pub fn name(&self) -> String {
        match self.about.name.trim() {
            "" if !self.is_pixel() => self.folder.clone(),
            name => name.to_string(),
        }
    }

    /// Its own folder, made if it is not there yet (Pixel's, the first time it keeps something):
    /// the wearer's alone (0700).
    pub fn own_dir(&self) -> Result<PathBuf, String> {
        if !self.dir.is_dir() {
            let mut make = std::fs::DirBuilder::new();
            make.recursive(true);
            #[cfg(unix)]
            std::os::unix::fs::DirBuilderExt::mode(&mut make, 0o700);
            make.create(&self.dir).map_err(|e| format!("{}: {e}", self.dir.display()))?;
        }
        Ok(self.dir.clone())
    }

    /// Where the moves made for it from words are kept, `<name>.json` each (`crate::moves`): in
    /// its own folder, Pixel's too (made with [`Self::own_dir`] before the first is written).
    pub fn moves_dir(&self) -> PathBuf {
        self.dir.join("moves")
    }

    /// Where the poses made for it from words are kept, likewise.
    pub fn poses_dir(&self) -> PathBuf {
        self.dir.join("poses")
    }

    /// The pal in `folder`.
    pub fn load(folder: &str) -> Result<Pack, String> {
        Pack::load_at(&root(), folder)
    }

    /// The pal in `folder` of `root`.
    pub fn load_at(root: &Path, folder: &str) -> Result<Pack, String> {
        if folder == PIXEL {
            return Err(format!("{PIXEL} is Pixel's own folder, not a pal's"));
        }
        let dir = root.join(folder);
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
        let offers = offers_of(figure.as_deref(), folder);
        let look = look_in(&dir);
        // Its own to change from its settings only where every file a change writes may be
        // written: the folder (a file is written beside itself and put in its place), pal.json,
        // and its persona's file if it has one.
        let persona_file = Some(dir.join(about.persona.trim())).filter(|p| !about.persona.trim().is_empty() && p.is_file());
        let writable = may_write(&dir) && may_write(&about_path) && persona_file.as_deref().is_none_or(may_write);
        let mut pack = Pack { folder: folder.to_string(), dir, about, persona, figure, offers, look, writable, moves: 0, stamps: Vec::new() };
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
        Pack { folder: "made-up".into(), dir, about, persona: persona.to_string(), figure: None, offers, look: Look::default(), writable: true, moves: 0, stamps: Vec::new() }
    }

    /// The files it is made of, watched for changes: an installed pal's `pal.json`, its persona's
    /// file (if it is one) and its figure; Pixel's figure, its settings being SpatiPal's own.
    fn files(&self) -> Vec<PathBuf> {
        if self.is_pixel() {
            return self.figure.iter().cloned().collect();
        }
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

    /// The settings as this pal has them. Pixel's are SpatiPal's settings themselves. An installed
    /// pal has its own name, personality, figure and voice whatever the settings say -- none of
    /// them Pixel's: one its folder leaves out is the room's own (its folder's name, no more
    /// personality than SpatiPal's first line, the robot, the voice described in words of a
    /// SpatiPal never set up). The rest is SpatiPal's, the same for every pal.
    pub fn over(&self, s: &Settings) -> Settings {
        let mut s = s.clone();
        if self.is_pixel() {
            return s;
        }
        let a = &self.about;
        s.name = self.name();
        s.persona = self.persona.trim().to_string();
        s.character = if self.figure.is_some() { a.figure.trim().to_string() } else { "robot".into() };
        s.voice = a.voice.trim().to_string();
        s.described = match a.described.trim() {
            "" if s.voice.is_empty() => Settings::default().described,
            described => described.to_string(),
        };
        s
    }

    /// The settings to keep, from `edited` (the settings window's, this pal's lines and
    /// SpatiPal's): for Pixel, all of them; for an installed pal, SpatiPal's, with Pixel's own
    /// left as they are, `saved` -- the pal's are written into its folder ([`Self::edit`]).
    pub fn under(&self, edited: &Settings, saved: &Settings) -> Settings {
        let mut s = edited.clone();
        if !self.is_pixel() {
            s.name = saved.name.clone();
            s.persona = saved.persona.clone();
            s.character = saved.character.clone();
            s.voice = saved.voice.clone();
            s.described = saved.described.clone();
        }
        s
    }

    /// An installed pal's own lines as changed in its settings window (`edited`, against what
    /// the window showed, `shown`), written into its folder: its name and voice into `pal.json`,
    /// every other field there kept as it was and where it was; its personality into its
    /// persona's file (`persona.md` made for one that had none), its maker's notes in it kept.
    /// Written the wearer's alone (0600), through a file beside it; the folder is left as it is.
    /// Whether anything was written. A pal whose files cannot be written is not changed.
    pub fn edit(&mut self, edited: &Settings, shown: &Settings) -> Result<bool, String> {
        if self.is_pixel() {
            return Ok(false);
        }
        let mut fields: Vec<(&str, String)> = Vec::new();
        if edited.name.trim() != shown.name.trim() && !edited.name.trim().is_empty() {
            fields.push(("name", edited.name.trim().to_string()));
        }
        if edited.voice.trim() != shown.voice.trim() {
            fields.push(("voice", edited.voice.trim().to_string()));
        }
        if edited.described.trim() != shown.described.trim() {
            fields.push(("described", edited.described.trim().to_string()));
        }
        let persona = (edited.persona.trim() != shown.persona.trim()).then(|| edited.persona.trim().to_string());
        if fields.is_empty() && persona.is_none() {
            return Ok(false);
        }
        if !self.writable {
            return Err(format!("{}'s files cannot be changed here: assistant/pals/{} is not writable.", self.name(), self.folder));
        }
        if let Some(words) = persona {
            let named = self.about.persona.trim().to_string();
            // Its persona's file; or, for one given in pal.json's own words, those words.
            let file = if named.is_empty() {
                Some("persona.md".to_string())
            } else {
                self.dir.join(&named).is_file().then_some(named)
            };
            match file {
                Some(file) => {
                    let path = self.dir.join(&file);
                    let was = std::fs::read_to_string(&path).unwrap_or_default();
                    crate::store::write(&path, &with_notes_of(&was, &words))?;
                    if self.about.persona.trim().is_empty() {
                        fields.push(("persona", file));
                    }
                }
                None => fields.push(("persona", words)),
            }
        }
        if !fields.is_empty() {
            let path = self.dir.join("pal.json");
            let was = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            crate::store::write(&path, &with_fields(&was, &fields).map_err(|e| format!("{} is not readable: {e}", path.display()))?)?;
        }
        Ok(true)
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

    /// Its groups of options, as offered: each with its options (the figure's own order).
    fn groups(&self) -> impl Iterator<Item = &(String, Vec<String>)> {
        self.offers.groups.iter().filter(|(g, _)| !OWN.contains(&g.as_str()))
    }

    /// Its moves made from words (`crate::moves`), kept in its folder -- its idle made among them.
    /// One of its moves (`pose`: false) or poses by this name, as it is named: its own or the
    /// figure's, or one made of words -- by the name itself, or by the only one whose name has
    /// all of its words ("hips": hands_on_hips; 2026-11-20, a small model making anew what it
    /// had, under a name of its own).
    pub fn known(&self, name: &str, pose: bool) -> Option<String> {
        let (moves, poses) = self.moves_and_poses();
        let names: Vec<String> = if pose { poses.into_iter().map(|(n, _)| n).chain(self.stock().into_iter().map(|(n, _, _)| n)).collect() } else { moves.into_iter().map(|(n, _)| n).collect() };
        let name = name.trim();
        if let Some(n) = names.iter().find(|n| n.eq_ignore_ascii_case(name)) {
            return Some(n.clone());
        }
        const SAID: [&str; 9] = ["pose", "move", "a", "the", "on", "your", "my", "with", "of"];
        let words = |n: &str| n.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty() && !SAID.contains(w)).map(str::to_string).collect::<Vec<_>>();
        let asked = words(name);
        if asked.is_empty() {
            return None;
        }
        let mut having = names.iter().filter(|n| {
            let theirs = words(n);
            asked.iter().all(|w| theirs.contains(w))
        });
        match (having.next(), having.next()) {
            (Some(n), None) => Some(n.clone()),
            _ => None,
        }
    }

    /// Its moves made from words, in its own folder ([`Self::moves_dir`]), Pixel's in Pixel's.
    pub fn shelf(&self) -> spatiand_voice::motion::Shelf {
        spatiand_voice::motion::Shelf { dir: self.moves_dir() }
    }

    /// Its poses made from words, held as its poses by name are ([`Self::poses_dir`]).
    pub fn pose_shelf(&self) -> spatiand_voice::motion::Shelf {
        spatiand_voice::motion::Shelf { dir: self.poses_dir() }
    }

    /// One of its moves or poses made from words, let go of -- or what it has made, when not.
    pub fn forget(&mut self, name: &str) -> Result<(), String> {
        let name = spatiand_voice::motion::name_of(name);
        if self.shelf().forget(&name).is_ok() || self.pose_shelf().forget(&name).is_ok() {
            if self.look.pose == name {
                self.look.pose.clear();
                self.keep_look();
            }
            return Ok(());
        }
        // A BVH the wearer gave it is theirs: it stays (`Shelf::forget` never takes one).
        if self.shelf().is_given(&name) || self.pose_shelf().is_given(&name) {
            return Err(format!("{name} was given to you by the wearer as a file, not made by you: it stays, unless they take it away"));
        }
        let made: Vec<String> = self.shelf().made().into_iter().chain(self.pose_shelf().made()).collect();
        Err(if made.is_empty() { format!("you have made no move {name}") } else { format!("you have made no move {name}: yours are {}", made.join(", ")) })
    }

    /// Its animations played once, and those held: each with its words, if `pal.json` has them;
    /// and the moves it made from words, played once too.
    fn moves_and_poses(&self) -> (Named, Named) {
        let (mut moves, mut poses) = (Vec::new(), Vec::new());
        // Its words in pal.json, for a BVH the wearer gave it; else what it is.
        let given_words = |map: &BTreeMap<String, String>, name: &str, what: &str| map.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.clone()).unwrap_or_else(|| what.to_string());
        let (shelf, pose_shelf) = (self.shelf(), self.pose_shelf());
        for kept in shelf.names() {
            if !STATES.contains(&kept.as_str()) {
                let words = if shelf.is_given(&kept) { given_words(&self.about.moves, &kept, "a move you were given") } else { "a move of your own, made from words".to_string() };
                moves.push((kept, words));
            }
        }
        for kept in pose_shelf.names() {
            let words = if pose_shelf.is_given(&kept) { given_words(&self.about.poses, &kept, "a pose you were given") } else { "a pose of your own, made from words".to_string() };
            poses.push((kept, words));
        }
        for name in &self.offers.animations {
            let bare = name.rsplit('|').next().unwrap_or(name);
            if STATES.iter().any(|s| s.eq_ignore_ascii_case(bare)) {
                continue;
            }
            let words = |map: &BTreeMap<String, String>| map.iter().find(|(k, _)| k.eq_ignore_ascii_case(bare)).map(|(_, v)| v.clone());
            match (words(&self.about.poses), words(&self.about.moves)) {
                // One of its own made of the same name is the one held.
                (Some(_), _) if poses.iter().any(|(n, _): &(String, String)| n.eq_ignore_ascii_case(bare)) => {}
                (Some(w), _) => poses.push((bare.to_string(), w)),
                // One of its own made of the same name is the one done.
                (None, _) if moves.iter().any(|(n, _): &(String, String)| n.eq_ignore_ascii_case(bare)) => {}
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
        let args = &self.mended(args);
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
                    // A model given only the names wrote one of its own (a small one writes its
                    // call as text at times, held to no list): told how to make it.
                    None => {
                        let made = if self.offers.body.is_empty() { "" } else { " -- or make it with make_move, kind move" };
                        return Err(format!("there is no move {value}: choose one of {}{made}", moves.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(", ")));
                    }
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
                        // As with a move: make_move, which both models have (own_pose is the
                        // larger's alone), 2026-11-20.
                        let made = if self.offers.body.is_empty() { "" } else { " -- or make it with make_move, kind pose" };
                        return Err(format!("there is no pose {value}: choose one of {}, or none{made}", all.join(", ")));
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

    /// A call as its model meant it: a pose by name written as one of `own_pose`'s parts (a
    /// model's slip, 2026-11-19: `{"own_pose": {"arms_folded": "true"}}`) is that pose by name;
    /// and a pose given as a move, or a move as a pose, is the one its name says (2026-11-20:
    /// "Now fold your arms." was `{"move": "arms_folded"}` in three of three).
    fn mended(&self, args: &Value) -> Value {
        let (moves, poses) = self.moves_and_poses();
        let stock = self.stock();
        let pose_named = |k: &str| poses.iter().map(|(n, _)| n.clone()).chain(stock.iter().map(|(n, _, _)| n.clone())).find(|n| n.eq_ignore_ascii_case(k.trim()));
        let move_named = |k: &str| moves.iter().map(|(n, _)| n.clone()).find(|n| n.eq_ignore_ascii_case(k.trim()));
        let mut mended = args.clone();
        let Some(o) = mended.as_object_mut() else { return mended };
        if let Some(name) = o.get("own_pose").and_then(Value::as_object).and_then(|parts| parts.keys().find_map(|k| pose_named(k))) {
            o.remove("own_pose");
            o.insert("pose".into(), json!(name));
        }
        let given = |o: &serde_json::Map<String, Value>, key: &str| o.get(key).and_then(Value::as_str).map(str::to_string);
        if let Some(m) = given(o, "move").filter(|m| move_named(m).is_none()) {
            if let (Some(p), false) = (pose_named(&m), o.contains_key("pose")) {
                o.remove("move");
                o.insert("pose".into(), json!(p));
            }
        }
        if let Some(p) = given(o, "pose").filter(|p| pose_named(p).is_none() && !p.eq_ignore_ascii_case("none")) {
            if let (Some(m), false) = (move_named(&p), o.contains_key("move")) {
                o.remove("pose");
                o.insert("move".into(), json!(m));
            }
        }
        mended
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
        let kept = serde_json::to_string_pretty(&self.look).map_err(|e| e.to_string()).and_then(|text| crate::store::write(&self.own_dir()?.join("look.json"), &text));
        if let Err(e) = kept {
            log::warn!("pal {}: how it looks could not be kept: {e}", self.key());
        }
    }

    /// Who it is to the room (the avatar's `character`): `pixel`, or `pal:<folder>`. The room
    /// remembers where the wearer put each one's figure by it.
    pub fn key(&self) -> String {
        if self.is_pixel() { PIXEL.to_string() } else { format!("pal:{}", self.folder) }
    }

    /// What the room is told of its figure: which file, how large, how it looks now. All of it,
    /// every time: what is left out the room keeps from before, which may be another pal's.
    pub fn figure_request(&self, shown: bool) -> Value {
        // Its moves and poses made from words, which the room plays as its animations.
        let mut motions = self.shelf().room();
        if let (Some(all), Value::Object(poses)) = (motions.as_object_mut(), self.pose_shelf().room()) {
            all.extend(poses);
        }
        let options: BTreeMap<&String, &String> = self.look.options.iter().filter(|(g, o)| self.offers.groups.iter().any(|(group, all)| group == *g && all.contains(o))).collect();
        json!({
            "do": "avatar",
            "character": self.key(),
            "path": self.figure,
            "visible": shown,
            "options": options,
            "pose": if self.look.own_pose.is_empty() { json!(self.look.pose) } else { json!(self.look.own_pose) },
            "expression": self.look.face,
            "colors": self.look.colours,
            "motions": motions,
            "size": if self.about.size > 0.0 { self.about.size } else { 1.0 },
            "signs": self.about.signs.unwrap_or(true),
        })
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
        // A small model, asked for a move it had not, said it could not do it, or said it let one go
        // without letting it go (neohorse-1:4b, 2026-11-19): said with every turn, where it weighs most.
        let body = if self.offers.body.is_empty() {
            ""
        } else {
            " [Your body can do anything asked of it: one of your moves or poses, or a new move with make_move -- never say you cannot; to let a move you made go, make_move with kind forget.]"
        };
        if parts.is_empty() {
            return body.trim().to_string();
        }
        format!("[How you look now: {}.]{body}", parts.join("; "))
    }
}

/// What a figure offers, read from its file; nothing for the room's robot (no file), or for a
/// file that says nothing of it.
fn offers_of(figure: Option<&Path>, who: &str) -> Offers {
    match figure {
        Some(path) => spatiand_voice::figure::read(path).unwrap_or_else(|e| {
            log::warn!("pal {who}: its figure says nothing of what it offers: {e}");
            Offers::default()
        }),
        None => Offers::default(),
    }
}

/// How a pal looked when it was last kept, in its own folder.
fn look_in(dir: &Path) -> Look {
    std::fs::read(dir.join("look.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// Whether this process may write `path` (a folder: make and replace files in it).
#[cfg(unix)]
fn may_write(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(name) = std::ffi::CString::new(path.as_os_str().as_bytes()) else { return false };
    unsafe { libc::access(name.as_ptr(), libc::W_OK) == 0 }
}

#[cfg(not(unix))]
fn may_write(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| !m.permissions().readonly())
}

/// A persona's file, its words changed to `words`: its maker's notes in it (`<!-- ... -->`,
/// which its model is never given) kept, before the words.
fn with_notes_of(was: &str, words: &str) -> String {
    let mut notes = Vec::new();
    let mut rest = was;
    while let Some(start) = rest.find("<!--") {
        let Some(end) = rest[start..].find("-->") else { break };
        notes.push(&rest[start..start + end + 3]);
        rest = &rest[start + end + 3..];
    }
    let words = words.trim();
    if notes.is_empty() { format!("{words}\n") } else { format!("{}\n\n{words}\n", notes.join("\n")) }
}

/// A pal.json's object, field by field in the order written, each value as it was written.
struct Fields(Vec<(String, Box<serde_json::value::RawValue>)>);

impl<'de> Deserialize<'de> for Fields {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Fields, D::Error> {
        struct Each;
        impl<'de> serde::de::Visitor<'de> for Each {
            type Value = Fields;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("an object")
            }
            fn visit_map<M: serde::de::MapAccess<'de>>(self, mut map: M) -> Result<Fields, M::Error> {
                let mut all = Vec::new();
                while let Some(field) = map.next_entry::<String, Box<serde_json::value::RawValue>>()? {
                    all.push(field);
                }
                Ok(Fields(all))
            }
        }
        d.deserialize_map(Each)
    }
}

/// `pal.json` with `fields` set to these words: each where it was, a new one at the end, and
/// every other field as it was written.
fn with_fields(was: &str, fields: &[(&str, String)]) -> Result<String, String> {
    let Fields(mut all) = serde_json::from_str(was).map_err(|e| e.to_string())?;
    for (name, words) in fields {
        let value = serde_json::value::to_raw_value(words).map_err(|e| e.to_string())?;
        match all.iter_mut().find(|(n, _)| n == name) {
            Some(field) => field.1 = value,
            None => all.push((name.to_string(), value)),
        }
    }
    let lines: Vec<String> = all.iter().map(|(name, value)| format!("  {}: {}", serde_json::to_string(name).unwrap_or_default(), value.get())).collect();
    Ok(format!("{{\n{}\n}}\n", lines.join(",\n")))
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
            writable: true,
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
        // A figure without a body is not told to make one.
        assert!(!p.apply(&json!({"pose": "dance"})).unwrap_err().contains("make_move"));
        assert!(!p.apply(&json!({"move": "cartwheel"})).unwrap_err().contains("make_move"));
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
        // A pose by name put among own_pose's parts, as a small model did: held by its name.
        let (room, _) = p.apply(&json!({"own_pose": {"arms_folded": "true"}})).unwrap();
        assert_eq!(room["pose"]["left_forearm"], "in, a little up");
        assert_eq!(p.look.pose, "arms_folded");
        // A pose given as a move, and a move as a pose: the one its name says.
        p.apply(&json!({"pose": "none"})).unwrap();
        let (room, did) = p.apply(&json!({"move": "Arms_Folded"})).unwrap();
        assert_eq!((p.look.pose.as_str(), room["pose"]["left_forearm"].as_str()), ("arms_folded", Some("in, a little up")));
        assert!(did.contains("pose: arms_folded"), "{did}");
        let (room, _) = p.apply(&json!({"pose": "wave"})).unwrap();
        assert_eq!((room["move"].as_str(), p.look.pose.as_str()), (Some("wave"), "arms_folded"));
        // Written as a string of JSON, as a small model may.
        let (room, _) = p.apply(&json!({"own_pose": "{\"head\": \"a little down\"}"})).unwrap();
        assert_eq!(room["pose"], json!({"head": "a little down"}));
        // What it has not, and words that say nothing, are answered with what would do.
        assert!(p.apply(&json!({"own_pose": {"left_shoulder": "up"}})).unwrap_err().contains("your parts are"));
        assert!(p.apply(&json!({"own_pose": {"left_hand": "on my hip"}})).unwrap_err().contains("front, back, up"));
        // One it has no name for, written by a small model in a call held to no list: told the
        // way it has to make it (2026-11-20: "kneel on one knee", told of own_pose, which it lacks).
        let why = p.apply(&json!({"pose": "kneel on one knee"})).unwrap_err();
        assert!(why.contains("hands_behind_back") && why.ends_with("or make it with make_move, kind pose"), "{why}");
        assert!(p.apply(&json!({"move": "cartwheel"})).unwrap_err().ends_with("make_move, kind move"));
        assert_eq!(p.look.own_pose.get("head").map(String::as_str), Some("a little down"));
        // One made of words, by its name: held as its words; one the figure has its own of is
        // its own; and one using a part it has not (shoulders) without it.
        let tool = p.tool().unwrap();
        let names = tool["function"]["parameters"]["properties"]["pose"]["enum"].clone();
        assert!(names.as_array().unwrap().iter().any(|n| n == "hands_on_hips"), "{names}");
        assert_eq!(p.known("Hands_Behind_Back", true).as_deref(), Some("hands_behind_back"));
        // By the only name that has all its words; none when several do, or none.
        assert_eq!(p.known("hips", true).as_deref(), Some("hands_on_hips"));
        assert_eq!(p.known("thinking pose", true).as_deref(), Some("thinking"));
        assert_eq!((p.known("hands_behind", true), p.known("hand_on_heart", true), p.known("kneel", true), p.known("pose", true)), (None, None, None, None));
        assert_eq!(names.as_array().unwrap().iter().filter(|n| *n == "hand_on_hip").count(), 1);
        // Either side of one that has a side, the right mirrored from the left.
        let (room, _) = p.apply(&json!({"pose": "look_over_right_shoulder"})).unwrap();
        assert_eq!((room["pose"]["head"].as_str(), room["pose"]["chest"].as_str()), (Some("right"), Some("turn right")));
        let (room, _) = p.apply(&json!({"pose": "right_hand_on_hip"})).unwrap();
        assert_eq!(room["pose"]["right_forearm"], "in, a little down, a little front");
        assert_eq!(room["pose"]["head"], "tilt left");
        let (room, _) = p.apply(&json!({"pose": "Hands_On_Hips"})).unwrap();
        assert_eq!(room["pose"]["left_forearm"], "in, a little down, a little front");
        assert!(p.note().starts_with("[How you look now: outfit casual; hair long; holding the pose hands_on_hips.] [Your body can do anything"), "{}", p.note());
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
    fn bvh_files_the_wearer_gave_it_are_offered_played_and_kept() {
        let mut about = about();
        about.moves.insert("happy_dance".into(), "a little happy dance".into());
        let mut p = pack(about, offers());
        std::fs::create_dir_all(p.moves_dir()).unwrap();
        std::fs::create_dir_all(p.poses_dir()).unwrap();
        std::fs::write(p.moves_dir().join("Happy Dance.bvh"), "HIERARCHY").unwrap();
        std::fs::write(p.moves_dir().join("cartwheel.bvh"), "HIERARCHY").unwrap();
        std::fs::write(p.poses_dir().join("Kneel.bvh"), "HIERARCHY").unwrap();
        std::fs::write(p.moves_dir().join("idle.bvh"), "HIERARCHY").unwrap();
        let tool = p.tool().unwrap();
        let props = &tool["function"]["parameters"]["properties"];
        let moves = props["move"]["description"].as_str().unwrap();
        // Its words in pal.json, or what it is; the idle is the room's, not a move.
        assert!(moves.contains("happy_dance (a little happy dance)") && moves.contains("cartwheel (a move you were given)"), "{moves}");
        assert!(!props["move"]["enum"].as_array().unwrap().iter().any(|n| n == "idle"));
        assert!(props["pose"]["description"].as_str().unwrap().contains("kneel (a pose you were given)"));
        // Told to the room by their own files, its idle among them.
        let room = p.figure_request(true);
        assert_eq!(room["motions"]["happy_dance"], json!(p.moves_dir().join("Happy Dance.bvh")));
        assert_eq!(room["motions"]["kneel"], json!(p.poses_dir().join("Kneel.bvh")));
        assert!(room["motions"].get("idle").is_some());
        let (room, _) = p.apply(&json!({"pose": "kneel"})).unwrap();
        assert_eq!(room["pose"], "kneel");
        // The wearer's: never let go of by its model.
        assert!(p.forget("cartwheel").unwrap_err().contains("given to you"));
        assert!(p.moves_dir().join("cartwheel.bvh").is_file());
        let _ = std::fs::remove_dir_all(&p.dir);
    }

    #[test]
    fn its_moves_made_from_words_are_offered_and_told() {
        let mut p = pack(about(), offers());
        let motion: spatiand_voice::motion::Motion = serde_json::from_value(json!({"fps": 30, "skeleton": {"joints": ["Pelvis"], "parents": [-1], "offsets": [[0, 0.97, 0]]}, "frames": [{"translation": [0, 0.97, 0], "rotations": [[0, 0, 0, 1]]}]})).unwrap();
        let ask = spatiand_voice::motion::Ask { prompt: "a person spins around once".into(), duration: 3.0, ..Default::default() };
        let twirl = p.shelf().keep("twirl", &ask, &motion).unwrap();
        // The same name as the figure's own `wave`: the one made is the one done.
        p.shelf().keep("wave", &ask, &motion).unwrap();
        // An idle made is the room's, not a move.
        p.shelf().keep("idle", &ask, &motion).unwrap();
        let tool = p.tool().unwrap();
        let moves = tool["function"]["parameters"]["properties"]["move"]["enum"].clone();
        assert_eq!(moves, json!(["twirl", "wave"]), "{moves}");
        let room = p.figure_request(true);
        assert_eq!(room["motions"]["twirl"], json!(twirl));
        assert!(room["motions"].get("idle").is_some());
        let (room, _) = p.apply(&json!({"move": "twirl"})).unwrap();
        assert_eq!((room["move"].as_str(), room["moves"].as_u64()), (Some("twirl"), Some(1)));
        // A pose made is held by its name, told to the room with the moves, and let go of.
        let one_leg = p.pose_shelf().keep("one_leg", &ask, &motion).unwrap();
        let tool = p.tool().unwrap();
        assert!(tool["function"]["parameters"]["properties"]["pose"]["enum"].as_array().unwrap().iter().any(|n| n == "one_leg"));
        let (room, _) = p.apply(&json!({"pose": "one_leg"})).unwrap();
        assert_eq!(room["pose"], "one_leg");
        assert_eq!(room["motions"]["one_leg"], json!(one_leg));
        // What it has by a name, as make_move asks it: a move as a move, a pose as a pose.
        assert_eq!(p.known("One_Leg", true).as_deref(), Some("one_leg"));
        assert_eq!(p.known("twirl", false).as_deref(), Some("twirl"));
        assert_eq!((p.known("one_leg", false), p.known("twirl", true), p.known("cartwheel", false)), (None, None, None));
        p.forget("one_leg").unwrap();
        assert_eq!(p.look.pose, "");
        assert!(p.forget("one_leg").unwrap_err().contains("yours are idle, twirl, wave"));
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

    /// A folder of pals of a test's own, as the wearer's would be.
    fn pals_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("spatipal-pals-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    /// A pal's folder made as docs/spatipal-pals.md asks: the folder 0700, its files 0600.
    fn installed(root: &Path, folder: &str, files: &[(&str, &str)]) -> PathBuf {
        let dir = root.join(folder);
        std::fs::create_dir_all(&dir).unwrap();
        for (name, text) in files {
            std::fs::write(dir.join(name), text).unwrap();
            #[cfg(unix)]
            std::fs::set_permissions(dir.join(name), std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
        }
        #[cfg(unix)]
        std::fs::set_permissions(&dir, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
        dir
    }

    #[test]
    fn pixel_is_the_settings_own_pal_and_first_and_none_is_gone() {
        let root = pals_root("pixel");
        installed(&root, "mira", &[("pal.json", r#"{"name": "Mira"}"#)]);
        // A pal.json in Pixel's own folder is not a pal; a folder without one is not either.
        installed(&root, PIXEL, &[("pal.json", r#"{"name": "Not Pixel"}"#)]);
        std::fs::create_dir_all(root.join("notes")).unwrap();
        assert_eq!(folders_in(&root), ["mira"]);
        assert!(Pack::load_at(&root, PIXEL).is_err());
        // The settings as an older SpatiPal kept them, with no pal chosen: they are Pixel's, all
        // of them, and nothing is changed by being Pixel's.
        let older = r#"{"on": true, "pal": "", "name": "Robo", "persona": "A grumpy old robot.", "character": "robot", "voice": "alba", "described": "", "spoken": false, "model": "gemma4:e4b"}"#;
        let saved: Settings = serde_json::from_str(older).unwrap();
        let pixel = Pack::pixel_at(&root, &saved);
        assert!(pixel.is_pixel() && pixel.writable);
        assert_eq!(pixel.over(&saved), saved);
        assert_eq!((pixel.name(), pixel.persona.as_str()), ("Robo".to_string(), "A grumpy old robot."));
        let mut edited = saved.clone();
        edited.name = "Robby".into();
        assert_eq!(pixel.under(&edited, &saved), edited);
        // Told to the room as itself, the robot, and all of it: size and signs too, which a pal
        // chosen before would otherwise have left there.
        let room = pixel.figure_request(true);
        assert_eq!(room["character"], "pixel");
        assert!(room["path"].is_null());
        assert_eq!((room["size"].as_f64(), room["signs"].as_bool()), (Some(1.0), Some(true)));
        // Its own folder, for its look and its moves, made only when first wanted.
        assert_eq!(pixel.moves_dir(), root.join(PIXEL).join("moves"));
        let _ = std::fs::remove_dir_all(root.join(PIXEL));
        assert!(!root.join(PIXEL).exists());
        assert_eq!(pixel.own_dir().unwrap(), root.join(PIXEL));
        #[cfg(unix)]
        assert_eq!(std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(root.join(PIXEL)).unwrap().permissions()) & 0o777, 0o700);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_installed_pal_has_its_own_name_personality_figure_and_voice() {
        let root = pals_root("own");
        installed(&root, "mira", &[("pal.json", r#"{"name": "Mira", "persona": "persona.md", "voice": "alba", "size": 1.6}"#), ("persona.md", "You are Mira.")]);
        installed(&root, "nameless", &[("pal.json", "{}")]);
        let saved: Settings = serde_json::from_str(r#"{"name": "Robo", "persona": "A grumpy old robot.", "character": "hero.vrm", "voice": "", "described": "A deep slow voice."}"#).unwrap();
        let mira = Pack::load_at(&root, "mira").unwrap();
        let s = mira.over(&saved);
        assert_eq!((s.name.as_str(), s.persona.as_str(), s.voice.as_str()), ("Mira", "You are Mira.", "alba"));
        // No figure of its own: the robot, not Pixel's.
        assert_eq!(s.character, "robot");
        assert_eq!(mira.figure_request(true)["character"], "pal:mira");
        assert!((mira.figure_request(true)["size"].as_f64().unwrap() - 1.6).abs() < 1e-6);
        // What its folder leaves out is the room's own, never Pixel's.
        let nameless = Pack::load_at(&root, "nameless").unwrap();
        let s = nameless.over(&saved);
        assert_eq!((s.name.as_str(), s.persona.as_str(), s.voice.as_str()), ("nameless", "", ""));
        assert_eq!(s.described, Settings::default().described);
        assert_ne!(s.described, saved.described);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_installed_pal_edited_writes_its_own_folder_and_keeps_the_rest() {
        let root = pals_root("edit");
        let about = "{\n  \"name\": \"Mira\",\n  \"private\": true,\n  \"persona\": \"persona.md\",\n  \"options\": {\"outfit\": {\"dress\": \"a short dress\", \"casual\": \"jeans\"}},\n  \"zeta\": [1, 2],\n  \"figure\": \"mira.glb\"\n}\n";
        let dir = installed(&root, "mira", &[("pal.json", about), ("persona.md", "<!-- my notes -->\nYou are Mira.\n\nYou tease.\n")]);
        let saved = Settings::default();
        let mut mira = Pack::load_at(&root, "mira").unwrap();
        assert!(mira.writable);
        let shown = mira.over(&saved);
        assert_eq!(shown.persona, "You are Mira.\n\nYou tease.");
        let mut edited = shown.clone();
        edited.name = "Mirabel".into();
        edited.voice = "alba".into();
        edited.persona = "You are Mirabel.\n\nYou tease, kindly.".into();
        edited.spoken = !shown.spoken;
        assert_eq!(mira.edit(&edited, &shown), Ok(true));
        // Its other fields kept, in their order and as written; its own changed; one new at the end.
        let written = std::fs::read_to_string(dir.join("pal.json")).unwrap();
        let Fields(fields) = serde_json::from_str(&written).unwrap();
        let names: Vec<&str> = fields.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["name", "private", "persona", "options", "zeta", "figure", "voice"]);
        assert_eq!(fields[3].1.get(), r#"{"outfit": {"dress": "a short dress", "casual": "jeans"}}"#);
        let about: About = serde_json::from_str(&written).unwrap();
        assert_eq!((about.name.as_str(), about.voice.as_str(), about.private), ("Mirabel", "alba", true));
        // Its personality in its persona's file, its maker's notes still there.
        assert_eq!(std::fs::read_to_string(dir.join("persona.md")).unwrap(), "<!-- my notes -->\n\nYou are Mirabel.\n\nYou tease, kindly.\n");
        let again = Pack::load_at(&root, "mira").unwrap();
        assert_eq!(again.over(&saved).persona, "You are Mirabel.\n\nYou tease, kindly.");
        assert!(again.about.private);
        // The wearer's own alone: the folder and its files as they were.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: PathBuf| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!((mode(dir.clone()), mode(dir.join("pal.json")), mode(dir.join("persona.md"))), (0o700, 0o600, 0o600));
        }
        // SpatiPal's own settings are kept as the window changed them, Pixel's as they were.
        let kept = mira.under(&edited, &saved);
        assert_eq!((kept.name.as_str(), kept.voice.as_str(), kept.persona.as_str()), (saved.name.as_str(), saved.voice.as_str(), saved.persona.as_str()));
        assert_eq!(kept.spoken, edited.spoken);
        // Nothing changed of its own: nothing written.
        assert_eq!(mira.edit(&shown, &shown), Ok(false));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_personality_goes_where_the_pal_keeps_it() {
        let root = pals_root("persona");
        // None yet: a persona.md of its own, named in its pal.json.
        let ada = installed(&root, "ada", &[("pal.json", r#"{"name": "Ada"}"#)]);
        let mut pack = Pack::load_at(&root, "ada").unwrap();
        let shown = pack.over(&Settings::default());
        let edited = Settings { persona: "You are Ada, precise and dry.".into(), ..shown.clone() };
        assert_eq!(pack.edit(&edited, &shown), Ok(true));
        assert_eq!(std::fs::read_to_string(ada.join("persona.md")).unwrap(), "You are Ada, precise and dry.\n");
        assert_eq!(serde_json::from_str::<About>(&std::fs::read_to_string(ada.join("pal.json")).unwrap()).unwrap().persona, "persona.md");
        #[cfg(unix)]
        assert_eq!(std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(ada.join("persona.md")).unwrap().permissions()) & 0o777, 0o600);
        // Given in pal.json's own words: changed there.
        let bo = installed(&root, "bo", &[("pal.json", r#"{"name": "Bo", "persona": "You are Bo."}"#)]);
        let mut pack = Pack::load_at(&root, "bo").unwrap();
        let shown = pack.over(&Settings::default());
        let edited = Settings { persona: "You are Bo, slow and kind.".into(), ..shown.clone() };
        assert_eq!(pack.edit(&edited, &shown), Ok(true));
        assert!(!bo.join("persona.md").exists());
        assert_eq!(Pack::load_at(&root, "bo").unwrap().persona, "You are Bo, slow and kind.");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn a_pal_whose_files_cannot_be_written_says_so_and_is_left_as_it_is() {
        use std::os::unix::fs::PermissionsExt;
        // Whoever may write anything may write this too: only as someone else is it refused.
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let root = pals_root("readonly");
        let about = r#"{"name": "Mira", "private": true}"#;
        let dir = installed(&root, "mira", &[("pal.json", about)]);
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500)).unwrap();
        let mut mira = Pack::load_at(&root, "mira").unwrap();
        assert!(!mira.writable);
        let shown = mira.over(&Settings::default());
        let edited = Settings { name: "Mirabel".into(), ..shown.clone() };
        let why = mira.edit(&edited, &shown).unwrap_err();
        assert!(why.contains("cannot be changed"), "{why}");
        assert_eq!(std::fs::read_to_string(dir.join("pal.json")).unwrap(), about);
        // A pal.json of its own made read-only is the same.
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::set_permissions(dir.join("pal.json"), std::fs::Permissions::from_mode(0o400)).unwrap();
        assert!(!Pack::load_at(&root, "mira").unwrap().writable);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_pal_chosen_before_stays_chosen() {
        let root = pals_root("chosen");
        installed(&root, "mira", &[("pal.json", r#"{"name": "Mira"}"#)]);
        // The settings as the owner's are: a pal chosen, or none -- which is Pixel now.
        let chose: Settings = serde_json::from_str(r#"{"on": true, "pal": "mira", "name": "Pixel"}"#).unwrap();
        assert_eq!(chose.pal, "mira");
        assert_eq!(Pack::load_at(&root, &chose.pal).unwrap().name(), "Mira");
        for none in [r#"{"on": true, "pal": "", "name": "Pixel"}"#, r#"{"on": true, "name": "Pixel"}"#] {
            let s: Settings = serde_json::from_str(none).unwrap();
            assert!(s.pal.is_empty());
            assert!(Pack::pixel_at(&root, &s).is_pixel());
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
