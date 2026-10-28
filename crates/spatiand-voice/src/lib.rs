//! Speech for Spatiand: dictation into whatever has focus, and later the assistant's ears and
//! voice.
//!
//! Nothing here listens or speaks by itself. The models all run on **hemera**, the model router
//! on deepmagpie (its own notes: `docs/integrate-spatiand.md` in the hemera repository), and the
//! microphone and the speakers belong to the compositor of each platform. What is here is what
//! sits between them, the same on the Deck, the Mac and the Beam Pro: cutting the microphone into
//! phrases, asking hemera what was said, steering it with what has focus, and turning spoken
//! symbols and near-miss names into what was meant. See `docs/ai-integrations.md`.

pub mod context;
pub mod dictation;
pub mod hemera;
pub mod phrases;
pub mod settings;
pub mod spoken;
pub mod words;

pub use context::Description;
pub use dictation::{Dictation, Event};
pub use hemera::Hemera;
pub use settings::VoiceSettings;

/// The rate everything here works at: Whisper's own, so nothing is resampled twice.
pub const RATE: u32 = 16_000;
