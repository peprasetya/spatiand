//! What the wearer chose for speech: kept with the applications' settings, so every linked
//! machine has the same (`~/.config/spatiand/apps/voice.json`, synchronised by spatiand-sync).

use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct VoiceSettings {
    /// hemera's address.
    pub hemera: String,
    /// The languages the wearer speaks, as Whisper names them. **Not a choice made per phrase**:
    /// with one, Whisper is told it; with several, Whisper hears which -- and a sentence that
    /// mixes them stays mixed.
    pub languages: Vec<String>,
    pub transcription_model: String,
    /// The small model that turns a phrase said into a terminal into what to type.
    pub cleanup_model: String,
    /// The wearer's own names and words, put right when Whisper hears them nearly.
    pub words: Vec<String>,
    /// How long a pause ends a phrase, in milliseconds.
    pub pause_ms: u32,
}

impl Default for VoiceSettings {
    fn default() -> Self {
        VoiceSettings {
            hemera: "http://deepmagpie:12434".into(),
            languages: vec!["en".into(), "id".into()],
            transcription_model: "whisper-large-v3-turbo".into(),
            cleanup_model: "neohorse-1:4b".into(),
            words: [
                "Spatiand", "SpatiQuill", "SpatiShelf", "SpatiReel", "SpatiLoupe", "SpatiMesh", "SpatiWorld", "Steam Deck",
                "Beam Pro", "deepmagpie", "hemera",
            ]
            .iter()
            .map(|w| w.to_string())
            .collect(),
            pause_ms: 700,
        }
    }
}

impl VoiceSettings {
    pub const FILE: &'static str = "voice.json";

    /// The settings in `folder`, or the defaults when there are none (or they do not read).
    pub fn load(folder: &Path) -> VoiceSettings {
        std::fs::read(folder.join(Self::FILE))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, folder: &Path) -> Result<(), String> {
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        let path = folder.join(Self::FILE);
        let partial = folder.join(format!(".{}.part", Self::FILE));
        std::fs::write(&partial, text).and_then(|_| std::fs::rename(&partial, &path)).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// A sentence in the way the wearer speaks, for Whisper's prompt before they have said
    /// anything: punctuated, and mixing their languages as they do.
    pub fn primer(&self) -> &'static str {
        let has = |l: &str| self.languages.iter().any(|x| x == l);
        match (has("en"), has("id")) {
            (true, true) => "Oke, aku cek dulu ya. Nanti meeting-nya jam tiga, terus aku push ke branch main. Can you check the log?",
            (false, true) => "Oke, saya cek dulu ya. Nanti rapatnya jam tiga, setelah itu saya kirim hasilnya.",
            (true, false) => "Okay, let me check that first. The meeting is at three, and then I'll push to the main branch.",
            _ => "",
        }
    }

    /// The language to tell Whisper, if there is only one to tell.
    pub fn whisper_language(&self) -> Option<&str> {
        match self.languages.as_slice() {
            [one] => Some(one.as_str()),
            _ => None,
        }
    }
}
