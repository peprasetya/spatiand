//! What SpatiPal keeps: its settings, the chats, the reminders. All in the folder every linked
//! machine shares (`~/.config/spatiand/apps`, synchronised by spatiand-sync), so a chat begun on
//! the Mac goes on on the Deck, and a reminder set on the Beam Pro is said on whichever is in use.
//!
//! One file per chat: the synchronisation takes the newer of each file, and two machines talking
//! in two chats at once must not overwrite each other. Files are written whole, through a
//! temporary, so a machine that is synchronising never reads half of one.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// The shared folder, as `spatiand_app::config` keeps it.
pub fn shared() -> PathBuf {
    spatiand_app::config::shared()
}

fn folder() -> PathBuf {
    let dir = shared().join("assistant");
    let _ = std::fs::create_dir_all(dir.join("chats"));
    dir
}

/// Seconds since 1970: when things were said and are due.
pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn write(path: &Path, text: &str) -> Result<(), String> {
    let partial = path.with_extension("part");
    std::fs::write(&partial, text).and_then(|_| std::fs::rename(&partial, path)).map_err(|e| format!("{}: {e}", path.display()))
}

// --- settings ---

/// What the wearer chose. `assistant.json`; the room reads `on` to start the assistant with the
/// session (`crate::assist` in the compositor).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    /// Started with the room, in the background.
    pub on: bool,
    /// What it answers to, and calls itself.
    pub name: String,
    /// Who it is, in its own words.
    pub persona: String,
    /// Its figure: `robot` (built in) or a file in `assistant/characters/`.
    pub character: String,
    /// A voice from hemera's library, by name; when empty, `described`.
    pub voice: String,
    /// A voice described in words, for hemera to make up.
    pub described: String,
    /// The small model: every request goes to it first.
    pub model: String,
    /// The larger model, for what the small one hands over.
    pub large_model: String,
    pub speech_model: String,
    /// While the larger model thinks: `quiet` (the figure shows it), `tone` (and a soft sound),
    /// `read` (its thinking read aloud as it comes).
    pub thinking: String,
    /// A conversation goes on in the last chat when it was this many hours ago or less; 0 for a
    /// new chat every time, a large number to always go on.
    pub continue_hours: u32,
    /// Listening all the time for its name, or only when asked.
    pub listening: bool,
    /// Answers spoken aloud, or only written.
    pub spoken: bool,
    /// Its figure in the room; hidden, it still hears and answers.
    pub shown: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            on: false,
            name: "Pixel".into(),
            persona: "A cheerful, quick little robot who likes helping and keeps it short.".into(),
            character: "robot".into(),
            voice: String::new(),
            described: "A friendly young voice, light and warm, speaking at an easy pace.".into(),
            model: "neohorse-1:4b".into(),
            large_model: "Qwen3.8:27b-q3".into(),
            speech_model: "voxcpm2".into(),
            thinking: "tone".into(),
            continue_hours: 4,
            listening: true,
            spoken: true,
            shown: true,
        }
    }
}

impl Settings {
    pub fn load() -> Settings {
        std::fs::read(shared().join("assistant.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        write(&shared().join("assistant.json"), &serde_json::to_string_pretty(self).map_err(|e| e.to_string())?)
    }
}

// --- chats ---

/// One thing said in a chat, as shown and as given back to the model.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Said {
    /// `user` or `assistant`.
    pub role: String,
    pub text: String,
    /// The larger model's thinking, when it was the one that answered.
    pub thinking: String,
    /// What it did in the room, in a few words each: "opened SpatiQuill".
    pub did: Vec<String>,
    /// Which model answered.
    pub model: String,
    pub at: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Chat {
    pub id: String,
    pub title: String,
    pub said: Vec<Said>,
    pub updated: u64,
}

impl Chat {
    pub fn new() -> Chat {
        let at = now();
        Chat { id: format!("{at}-{:04x}", std::process::id() & 0xffff), title: String::new(), said: Vec::new(), updated: at }
    }

    pub fn save(&self) -> Result<(), String> {
        write(&folder().join("chats").join(format!("{}.json", self.id)), &serde_json::to_string(self).map_err(|e| e.to_string())?)
    }

    pub fn forget(&self) {
        let _ = std::fs::remove_file(folder().join("chats").join(format!("{}.json", self.id)));
    }
}

/// Every chat, the most recent first.
pub fn chats() -> Vec<Chat> {
    let mut all: Vec<Chat> = std::fs::read_dir(folder().join("chats"))
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
                .filter_map(|e| std::fs::read(e.path()).ok())
                .filter_map(|b| serde_json::from_slice::<Chat>(&b).ok())
                .collect()
        })
        .unwrap_or_default();
    all.sort_by(|a, b| b.updated.cmp(&a.updated));
    all
}

// --- reminders ---

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Reminder {
    pub id: u64,
    pub text: String,
    /// When, in seconds since 1970.
    pub due: u64,
    /// Said already (on some machine).
    pub done: bool,
}

pub fn reminders() -> Vec<Reminder> {
    std::fs::read(folder().join("reminders.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

pub fn save_reminders(all: &[Reminder]) -> Result<(), String> {
    // Long said, long gone: kept a week for "what did you remind me of".
    let keep: Vec<&Reminder> = all.iter().filter(|r| !r.done || r.due + 7 * 86_400 > now()).collect();
    write(&folder().join("reminders.json"), &serde_json::to_string_pretty(&keep).map_err(|e| e.to_string())?)
}

/// The characters there are: the built-in robot, and the wearer's own figures (`.vrm`, `.glb`)
/// put in `assistant/characters/`.
pub fn characters() -> Vec<String> {
    let mut all = vec!["robot".to_string()];
    let dir = folder().join("characters");
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(entries) = std::fs::read_dir(dir) {
        let mut own: Vec<String> = entries
            .flatten()
            .filter_map(|e| e.file_name().to_str().map(str::to_string))
            .filter(|n| n.to_lowercase().ends_with(".vrm") || n.to_lowercase().ends_with(".glb"))
            .collect();
        own.sort();
        all.extend(own);
    }
    all
}

/// Where a character's file is, for the room to load.
pub fn character_path(name: &str) -> Option<PathBuf> {
    (name != "robot").then(|| folder().join("characters").join(name)).filter(|p| p.is_file())
}
