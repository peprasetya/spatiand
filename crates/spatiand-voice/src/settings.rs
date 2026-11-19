//! What speech needs of the wearer's choices: hemera, the languages and words dictation hears
//! with, and its two models. Taken from the AI settings (`spatiand_ai`, `ai.json` in the folder
//! every paired machine shares, changed on the room's AI card); this has no file of its own.
//! It had one, `voice.json`, until 2026-11-19, which the AI settings were carried over from.

use spatiand_ai::AiSettings;

#[derive(Clone, Debug, PartialEq)]
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
        VoiceSettings::of(&AiSettings::default())
    }
}

impl VoiceSettings {
    /// What speech needs of the AI settings.
    pub fn of(ai: &AiSettings) -> VoiceSettings {
        VoiceSettings {
            hemera: ai.hemera.clone(),
            languages: ai.dictation.languages.clone(),
            transcription_model: ai.models.transcription.clone(),
            cleanup_model: ai.models.cleanup.clone(),
            words: ai.dictation.words.clone(),
            pause_ms: ai.dictation.pause_ms,
        }
    }

    /// The AI settings kept in `folder` (the shared folder), as speech needs them.
    pub fn load(folder: &std::path::Path) -> VoiceSettings {
        VoiceSettings::of(&AiSettings::load(folder))
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
