//! The room's side of the AI card (`spatiand_shell::ai`): the settings it shows, kept in the file
//! every paired machine shares (`spatiand_ai`, `ai.json`), and what hemera offers its choices,
//! asked on a thread of its own each time the card opens -- the compositor's thread draws the
//! glasses, and hemera may be a Wi-Fi and a Tailscale away.
//!
//! The same on the Deck, the Mac and the Beam Pro: each backend calls [`Card::open`] when the
//! settings' AI row is chosen, [`Card::act`] with what the wearer changed, and [`Card::tick`]
//! each frame. Dictation reads the same file each time it starts (`crate::dictate`), and every
//! application through the SDK, which tells it when the file changes (`spatiand_app::ai`).

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, TryRecvError};

use spatiand_ai::AiSettings;
use spatiand_shell::ai::{AiAction, AiView, Model, Offered};
use spatiand_shell::Shell;

/// The folder every machine shares, as the synchronisation has it.
fn folder() -> PathBuf {
    spatiand_sync::default_folders().0
}

/// The card's errands: hemera being asked what it offers.
#[derive(Default)]
pub struct Card {
    asking: Option<Receiver<Offered>>,
}

impl Card {
    /// The card has opened: the settings as they are kept, and hemera asked what it offers.
    pub fn open(&mut self, shell: &mut Shell) {
        let settings = AiSettings::load(&folder());
        shell.set_ai(view_of(&settings));
        shell.set_ai_note(None);
        self.ask(&settings.hemera, shell);
    }

    fn ask(&mut self, hemera: &str, shell: &mut Shell) {
        shell.set_ai_offered(Offered { asking: true, ..Default::default() });
        let (tx, rx) = channel();
        let hemera = hemera.to_string();
        let spawned = std::thread::Builder::new().name("ai-card".into()).spawn(move || {
            let _ = tx.send(offered_by(&hemera));
        });
        match spawned {
            Ok(_) => self.asking = Some(rx),
            Err(e) => shell.set_ai_offered(Offered { trouble: Some(format!("hemera could not be asked ({e}).")), ..Default::default() }),
        }
    }

    /// Each frame: hemera's answer, once it has come.
    pub fn tick(&mut self, shell: &mut Shell) {
        let Some(asking) = &self.asking else { return };
        match asking.try_recv() {
            Ok(offered) => {
                shell.set_ai_offered(offered);
                self.asking = None;
            }
            Err(TryRecvError::Disconnected) => self.asking = None,
            Err(TryRecvError::Empty) => {}
        }
    }

    /// The wearer changed something: kept, and the card shown the settings as they now are. Read
    /// afresh first -- another machine's change may have come since the card opened -- and never
    /// saved over a file that cannot be read.
    pub fn act(&mut self, action: AiAction, shell: &mut Shell) {
        let folder = folder();
        let mut settings = match AiSettings::read(&folder) {
            Ok(settings) => settings,
            Err(why) => {
                log::warn!("ai: {why}");
                shell.set_ai_note(Some(format!("Nothing was changed: the settings could not be read ({why}).")));
                return;
            }
        };
        let hemera = settings.hemera.clone();
        apply(&mut settings, &action);
        match settings.save(&folder) {
            Ok(()) => {
                log::info!("ai: {action:?} kept");
                shell.set_ai_note(None);
                shell.set_ai(view_of(&settings));
                // Another hemera has other models: asked again.
                if settings.hemera != hemera {
                    self.ask(&settings.hemera, shell);
                }
            }
            Err(why) => {
                log::warn!("ai: {action:?} not kept: {why}");
                shell.set_ai_note(Some(format!("The change could not be kept ({why}).")));
                shell.set_ai(view_of(&AiSettings::load(&folder)));
            }
        }
    }
}

/// What the wearer changed, made to the settings.
pub fn apply(settings: &mut AiSettings, action: &AiAction) {
    match action {
        AiAction::Hemera(address) => settings.hemera = address.clone(),
        AiAction::Model(model, name) => {
            let slot = match model {
                Model::Everyday => &mut settings.models.everyday,
                Model::Larger => &mut settings.models.larger,
                Model::Cleanup => &mut settings.models.cleanup,
                Model::Reading => &mut settings.models.reading,
            };
            *slot = name.clone();
        }
        AiAction::Languages(languages) => settings.dictation.languages = languages.clone(),
        AiAction::AddWord(word) => {
            if !settings.dictation.words.iter().any(|w| w.eq_ignore_ascii_case(word)) {
                settings.dictation.words.push(word.clone());
            }
        }
        AiAction::ForgetWord(word) => settings.dictation.words.retain(|w| w != word),
        AiAction::ReadByItself(on) => settings.text_in_pictures.read_by_itself = *on,
        AiAction::TextLanguages(first, second) => {
            settings.text_in_pictures.first_language = first.clone();
            settings.text_in_pictures.second_language = second.clone();
        }
    }
}

/// The settings, as the card shows them.
pub fn view_of(s: &AiSettings) -> AiView {
    AiView {
        hemera: s.hemera.clone(),
        everyday: s.models.everyday.clone(),
        larger: s.models.larger.clone(),
        cleanup: s.models.cleanup.clone(),
        reading: s.models.reading.clone(),
        languages: s.dictation.languages.clone(),
        words: s.dictation.words.clone(),
        read_by_itself: s.text_in_pictures.read_by_itself,
        first_language: s.text_in_pictures.first_language.clone(),
        second_language: s.text_in_pictures.second_language.clone(),
    }
}

/// What hemera at `address` offers: its models that write text, as SpatiPal offered them, and
/// those that read text in pictures, with the one it reads with when none is named. Three GETs,
/// none of which starts a model.
fn offered_by(address: &str) -> Offered {
    let hemera = spatiand_voice::Hemera::new(address);
    match hemera.models("text") {
        Ok(models) => Offered {
            asking: false,
            models,
            readers: hemera.models("ocr").unwrap_or_default(),
            reader_default: hemera
                .get_json("/v1/ocr")
                .ok()
                .and_then(|v| v.get("default_model").and_then(serde_json::Value::as_str).map(str::to_string)),
            trouble: None,
        },
        Err(why) => {
            log::warn!("ai: {why}");
            Offered { trouble: Some(format!("{why}. Its models can be chosen once it answers.")), ..Default::default() }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_change_on_the_card_lands_where_the_apps_read_it() {
        let mut s = AiSettings::default();
        apply(&mut s, &AiAction::Hemera("http://elsewhere:12434".into()));
        apply(&mut s, &AiAction::Model(Model::Larger, "Ornith-1.5:35b".into()));
        apply(&mut s, &AiAction::Model(Model::Reading, "pp-ocrv6".into()));
        apply(&mut s, &AiAction::Languages(vec!["id".into()]));
        apply(&mut s, &AiAction::AddWord("Pixel".into()));
        apply(&mut s, &AiAction::AddWord("pixel".into()));
        apply(&mut s, &AiAction::ForgetWord("SpatiWorld".into()));
        apply(&mut s, &AiAction::ReadByItself(false));
        apply(&mut s, &AiAction::TextLanguages("id".into(), "en".into()));
        assert_eq!(s.hemera, "http://elsewhere:12434");
        assert_eq!(s.models.larger, "Ornith-1.5:35b");
        assert_eq!(s.models.reading, "pp-ocrv6");
        assert_eq!(s.models.everyday, AiSettings::default().models.everyday);
        assert_eq!(s.dictation.languages, vec!["id".to_string()]);
        assert_eq!(s.dictation.words.iter().filter(|w| w.eq_ignore_ascii_case("pixel")).count(), 1);
        assert!(!s.dictation.words.iter().any(|w| w == "SpatiWorld"));
        assert!(!s.text_in_pictures.read_by_itself);
        assert_eq!((s.text_in_pictures.first_language.as_str(), s.text_in_pictures.second_language.as_str()), ("id", "en"));
        // And the card shows what was kept.
        let v = view_of(&s);
        assert_eq!((v.hemera.as_str(), v.larger.as_str(), v.reading.as_str()), ("http://elsewhere:12434", "Ornith-1.5:35b", "pp-ocrv6"));
        assert_eq!(v.first_language, "id");
    }
}
