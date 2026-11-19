//! The AI settings: what more than one app uses -- hemera's address, the models, dictation, and
//! text in pictures. Changed in the room's settings, on its AI card (`spatiand_shell::ai`).
//!
//! **One file, `ai.json`, in the folder every paired machine shares** (`~/.config/spatiand/apps`,
//! synchronised by spatiand-sync), so what is chosen on one machine is chosen on all of them, as
//! SpatiPal's settings always were. The room reads it (dictation, the card); every application
//! reads it through the SDK (`spatiand_app::ai::settings`), which also says when it changed.
//!
//! Until 2026-11-19 these were SpatiPal's: hemera and dictation in `voice.json`, the everyday and
//! the larger model in `assistant.json`. The first read of a folder with no `ai.json` makes one
//! from those, once ([`AiSettings::load`]); after that they are not read again. They are left as
//! they are, for a machine still running an older build.

use std::path::Path;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Where they are: `<the shared folder>/ai.json`.
pub const FILE: &str = "ai.json";

/// The older builds' files they were carried over from.
const OLDER_VOICE: &str = "voice.json";
const OLDER_ASSISTANT: &str = "assistant.json";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AiSettings {
    /// hemera's address, `http://deepmagpie:12434`: every model and voice is there.
    pub hemera: String,
    pub models: Models,
    pub dictation: Dictation,
    pub text_in_pictures: TextInPictures,
    /// What a newer build put in the file and this one does not know, written back as it was.
    #[serde(flatten)]
    pub kept: Map<String, Value>,
}

/// hemera's models, by name, each for what it does.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Models {
    /// The quick one, which every request goes to first: the assistant's answers.
    pub everyday: String,
    /// For what needs thought: the everyday one hands it over.
    pub larger: String,
    /// Turns what is said into a terminal into the command to type.
    pub cleanup: String,
    /// Reads the words in pictures. Empty for hemera's own choice: no model is named to it.
    pub reading: String,
    /// Writes down what is said (Whisper). Not offered on the card.
    pub transcription: String,
    #[serde(flatten)]
    pub kept: Map<String, Value>,
}

/// What dictation and the assistant's ears hear with.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Dictation {
    /// The languages the wearer speaks, as Whisper names them. **Not a choice made per phrase**:
    /// with one, Whisper is told it; with several, Whisper hears which -- and a sentence that
    /// mixes them stays mixed. None: Whisper decides.
    pub languages: Vec<String>,
    /// The wearer's own names and words, put right when Whisper hears them nearly.
    pub words: Vec<String>,
    /// How long a pause ends a phrase, in milliseconds. Not offered on the card.
    pub pause_ms: u32,
    #[serde(flatten)]
    pub kept: Map<String, Value>,
}

/// For choosing text in pictures (the owner, 2026-11-19: "the selection in primary, translate to
/// secondary. other, translate to primary"). **Nothing reads text yet**: kept for that work.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct TextInPictures {
    /// A picture's words are read as soon as it is shown, ready to be chosen; else only when asked.
    pub read_by_itself: bool,
    /// Text in this language is translated into the second; anything else into this one. A
    /// language's two-letter name (`en`).
    pub first_language: String,
    pub second_language: String,
    #[serde(flatten)]
    pub kept: Map<String, Value>,
}

impl Default for AiSettings {
    fn default() -> Self {
        AiSettings {
            hemera: "http://deepmagpie:12434".into(),
            models: Models::default(),
            dictation: Dictation::default(),
            text_in_pictures: TextInPictures::default(),
            kept: Map::new(),
        }
    }
}

impl Default for Models {
    fn default() -> Self {
        Models {
            everyday: "neohorse-1:4b".into(),
            larger: "Qwen3.8:27b-q3".into(),
            cleanup: "neohorse-1:4b".into(),
            reading: String::new(),
            transcription: "whisper-large-v3-turbo".into(),
            kept: Map::new(),
        }
    }
}

impl Default for Dictation {
    fn default() -> Self {
        Dictation {
            languages: vec!["en".into(), "id".into()],
            words: [
                "Spatiand", "SpatiQuill", "SpatiShelf", "SpatiReel", "SpatiLoupe", "SpatiMesh", "SpatiWorld", "Steam Deck",
                "Beam Pro", "deepmagpie", "hemera",
            ]
            .iter()
            .map(|w| w.to_string())
            .collect(),
            pause_ms: 700,
            kept: Map::new(),
        }
    }
}

impl Default for TextInPictures {
    fn default() -> Self {
        TextInPictures { read_by_itself: true, first_language: "en".into(), second_language: "id".into(), kept: Map::new() }
    }
}

impl AiSettings {
    /// The settings in `folder` (the shared folder), or the defaults when they cannot be read.
    ///
    /// With no `ai.json` there yet, they are made from what an older build kept (see the crate's
    /// note) and written, so that this happens once.
    pub fn load(folder: &Path) -> AiSettings {
        Self::read(folder).unwrap_or_default()
    }

    /// As [`AiSettings::load`], but a file that is there and cannot be read is an error, **not**
    /// the defaults -- so that a change is not saved over it with the rest lost (half a file, as
    /// a synchronisation is under way).
    pub fn read(folder: &Path) -> Result<AiSettings, String> {
        let path = folder.join(FILE);
        match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| format!("{} is not readable ({e}): left as it is", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::carry_over(folder)),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    /// Kept in `folder`, whole, through a temporary: a machine synchronising never reads half.
    pub fn save(&self, folder: &Path) -> Result<(), String> {
        self.write(folder, None)
    }

    /// What an older build kept, in these settings' shape: `voice.json`'s fields as they were,
    /// and `assistant.json`'s `model` and `large_model`. What either lacks is the default.
    pub fn from_older(voice: Option<&Value>, assistant: Option<&Value>) -> AiSettings {
        let text = |file: Option<&Value>, key: &str| file.and_then(|f| f.get(key)).and_then(Value::as_str).map(str::to_string);
        let list = |file: Option<&Value>, key: &str| {
            file.and_then(|f| f.get(key)).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect::<Vec<_>>())
        };
        let mut s = AiSettings::default();
        if let Some(v) = text(voice, "hemera") {
            s.hemera = v;
        }
        if let Some(v) = list(voice, "languages") {
            s.dictation.languages = v;
        }
        if let Some(v) = list(voice, "words") {
            s.dictation.words = v;
        }
        if let Some(v) = voice.and_then(|f| f.get("pause_ms")).and_then(Value::as_u64) {
            s.dictation.pause_ms = v.min(u32::MAX as u64) as u32;
        }
        if let Some(v) = text(voice, "transcription_model") {
            s.models.transcription = v;
        }
        if let Some(v) = text(voice, "cleanup_model") {
            s.models.cleanup = v;
        }
        if let Some(v) = text(assistant, "model") {
            s.models.everyday = v;
        }
        if let Some(v) = text(assistant, "large_model") {
            s.models.larger = v;
        }
        s
    }

    /// The first read of a folder with no `ai.json`: made from an older build's files, and
    /// written **with their time, not now's** -- a machine whose older files were behind
    /// another's then has the older `ai.json`, and the other's wins when they synchronise. With
    /// neither file, the defaults, and nothing written: a machine new to the folder must not hand
    /// its defaults to every other as the newest settings. With one that cannot be read (half
    /// of it synchronised), what can be read, and nothing written yet: the next read tries again.
    fn carry_over(folder: &Path) -> AiSettings {
        let older = |name: &str| -> Result<Option<(Value, Option<SystemTime>)>, ()> {
            let path = folder.join(name);
            match std::fs::read(&path) {
                Ok(bytes) => {
                    let value = serde_json::from_slice::<Value>(&bytes).map_err(|_| ())?;
                    Ok(Some((value, std::fs::metadata(&path).and_then(|m| m.modified()).ok())))
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(_) => Err(()),
            }
        };
        let (voice, assistant) = (older(OLDER_VOICE), older(OLDER_ASSISTANT));
        let readable = voice.is_ok() && assistant.is_ok();
        let (voice, assistant) = (voice.ok().flatten(), assistant.ok().flatten());
        let settings = Self::from_older(voice.as_ref().map(|(v, _)| v), assistant.as_ref().map(|(a, _)| a));
        if readable && (voice.is_some() || assistant.is_some()) {
            let when = [&voice, &assistant].into_iter().flatten().filter_map(|(_, at)| *at).max();
            let _ = settings.write(folder, when);
        }
        settings
    }

    /// Written whole, through a temporary of this process's own, **the wearer's alone** (0600:
    /// their words are in it), dated `when` if given.
    fn write(&self, folder: &Path, when: Option<SystemTime>) -> Result<(), String> {
        use std::io::Write;
        let path = folder.join(FILE);
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        // A dot first: the synchronisation never sends a name that begins with one.
        let partial = folder.join(format!(".{FILE}.{}.part", std::process::id()));
        let written = (|| {
            std::fs::create_dir_all(folder)?;
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create(true).truncate(true);
            #[cfg(unix)]
            std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
            let mut file = options.open(&partial)?;
            #[cfg(unix)]
            file.set_permissions(<std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o600))?;
            file.write_all(text.as_bytes())?;
            if let Some(when) = when {
                file.set_modified(when)?;
            }
            drop(file);
            std::fs::rename(&partial, &path)
        })();
        if written.is_err() {
            let _ = std::fs::remove_file(&partial);
        }
        written.map_err(|e| format!("{}: {e}", path.display()))
    }
}

/// When `folder`'s settings last changed, and how long the file is: what a reader compares to
/// know they changed without reading them. `None` while there is no file.
pub fn stamp(folder: &Path) -> Option<(SystemTime, u64)> {
    let meta = std::fs::metadata(folder.join(FILE)).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("spatiand-ai-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The owner's two files as SpatiPal wrote them, every value his own.
    fn older_files(dir: &Path) {
        std::fs::write(
            dir.join(OLDER_VOICE),
            r#"{"hemera": "http://hemera.example:9000", "languages": ["id"], "transcription_model": "whisper-large-v3",
                "cleanup_model": "ornith-1.5:9b", "words": ["Pixel", "Spatiand"], "pause_ms": 900}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join(OLDER_ASSISTANT),
            r#"{"on": true, "name": "Pixel", "model": "gemma4:e4b", "large_model": "Ornith-1.5:35b", "spoken": false}"#,
        )
        .unwrap();
    }

    #[test]
    fn an_older_builds_settings_are_carried_over_once_and_nothing_is_lost() {
        let dir = folder("carried");
        older_files(&dir);
        // Dated as they were changed, not as the carrying over was done.
        let changed = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_763_000_000);
        for name in [OLDER_VOICE, OLDER_ASSISTANT] {
            std::fs::File::options().write(true).open(dir.join(name)).unwrap().set_modified(changed).unwrap();
        }
        let s = AiSettings::load(&dir);
        assert_eq!(s.hemera, "http://hemera.example:9000");
        assert_eq!(s.dictation.languages, vec!["id".to_string()]);
        assert_eq!(s.dictation.words, vec!["Pixel".to_string(), "Spatiand".into()]);
        assert_eq!(s.dictation.pause_ms, 900);
        assert_eq!(s.models.transcription, "whisper-large-v3");
        assert_eq!(s.models.cleanup, "ornith-1.5:9b");
        assert_eq!(s.models.everyday, "gemma4:e4b");
        assert_eq!(s.models.larger, "Ornith-1.5:35b");
        // What the older files never had is the default.
        assert_eq!(s.models.reading, "");
        assert_eq!(s.text_in_pictures, TextInPictures::default());
        // Written, once, with the older files' time; they are left as they were.
        assert_eq!(stamp(&dir).unwrap().0, changed);
        assert_eq!(AiSettings::read(&dir).unwrap(), s);
        assert!(std::fs::read_to_string(dir.join(OLDER_ASSISTANT)).unwrap().contains("gemma4:e4b"));
        // Changed later in an older build: not carried again.
        std::fs::write(dir.join(OLDER_ASSISTANT), r#"{"model": "something-else"}"#).unwrap();
        assert_eq!(AiSettings::load(&dir).models.everyday, "gemma4:e4b");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_store_that_has_values_is_not_overwritten() {
        let dir = folder("kept");
        let mut mine = AiSettings::default();
        mine.hemera = "http://elsewhere:12434".into();
        mine.models.reading = "pp-ocrv6".into();
        mine.text_in_pictures.second_language = "ja".into();
        mine.save(&dir).unwrap();
        let before = stamp(&dir);
        older_files(&dir);
        assert_eq!(AiSettings::load(&dir), mine);
        assert_eq!(stamp(&dir), before, "the store is not written again");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_folder_new_to_the_settings_gets_the_defaults_and_writes_nothing() {
        let dir = folder("new");
        assert_eq!(AiSettings::load(&dir), AiSettings::default());
        assert!(stamp(&dir).is_none(), "defaults written would be newer than every other machine's");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn half_an_older_file_is_not_carried_over_until_it_is_whole() {
        let dir = folder("half");
        older_files(&dir);
        std::fs::write(dir.join(OLDER_VOICE), r#"{"hemera": "http://hem"#).unwrap();
        let s = AiSettings::load(&dir);
        assert_eq!(s.models.everyday, "gemma4:e4b", "what can be read is used");
        assert!(stamp(&dir).is_none(), "and nothing is written yet");
        older_files(&dir);
        assert_eq!(AiSettings::load(&dir).hemera, "http://hemera.example:9000");
        assert!(stamp(&dir).is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_store_that_cannot_be_read_is_an_error_and_is_left_alone() {
        let dir = folder("broken");
        std::fs::write(dir.join(FILE), r#"{"hemera": "#).unwrap();
        assert!(AiSettings::read(&dir).is_err());
        assert_eq!(AiSettings::load(&dir), AiSettings::default());
        assert_eq!(std::fs::read_to_string(dir.join(FILE)).unwrap(), r#"{"hemera": "#);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn what_a_newer_build_added_is_written_back_as_it_was() {
        let dir = folder("newer");
        std::fs::write(
            dir.join(FILE),
            r#"{"hemera": "http://h:1", "models": {"everyday": "a", "speaking": "voxcpm3"}, "later": {"x": 1}}"#,
        )
        .unwrap();
        let mut s = AiSettings::load(&dir);
        assert_eq!(s.models.everyday, "a");
        assert_eq!(s.models.larger, Models::default().larger);
        s.hemera = "http://h:2".into();
        s.save(&dir).unwrap();
        let written: Value = serde_json::from_str(&std::fs::read_to_string(dir.join(FILE)).unwrap()).unwrap();
        assert_eq!(written["hemera"], "http://h:2");
        assert_eq!(written["models"]["speaking"], "voxcpm3");
        assert_eq!(written["later"]["x"], 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn the_store_is_the_wearer_s_alone_and_leaves_no_temporary() {
        use std::os::unix::fs::PermissionsExt;
        let dir = folder("mode");
        AiSettings::default().save(&dir).unwrap();
        assert_eq!(std::fs::metadata(dir.join(FILE)).unwrap().permissions().mode() & 0o777, 0o600);
        let names: Vec<String> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(names, vec![FILE.to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
