//! What the room does for SpatiKey: type into a window, say which window, show a card.
//!
//! The compositor sets this when it starts (on the Mac, when the room is up; on the Deck and the
//! Beam Pro, always) and clears it when it ends. The service asks it; with none set, typing is
//! refused with `not_here`.

use std::sync::{Arc, OnceLock, RwLock};

use zeroize::Zeroizing;

pub trait Room: Send + Sync {
    /// The window the wearer was in before SpatiKey's own, or the focused one if SpatiKey has
    /// none open. None when there is no such window.
    fn target(&self) -> Option<Target>;
    /// Type into `window`. Refuses before sending any key if it cannot type all of it, or if the
    /// keyboard focus did not land on that window.
    fn type_into(&self, window: u64, steps: &[Step]) -> Result<(), String>;
    /// A card in the room ("Touch ID on darkPhoenix to sign in to github.com"); an empty text
    /// takes it away.
    fn notice(&self, text: &str);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub window: u64,
    pub app: String,
    pub title: String,
}

#[derive(Clone, PartialEq, Eq)]
pub enum Step {
    Text(Zeroizing<String>),
    Tab,
    Enter,
}

impl std::fmt::Debug for Step {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Step::Text(text) => write!(f, "Text({} characters)", text.chars().count()),
            Step::Tab => write!(f, "Tab"),
            Step::Enter => write!(f, "Enter"),
        }
    }
}

fn slot() -> &'static RwLock<Option<Arc<dyn Room>>> {
    static ROOM: OnceLock<RwLock<Option<Arc<dyn Room>>>> = OnceLock::new();
    ROOM.get_or_init(|| RwLock::new(None))
}

/// Set (or with None, clear) the room.
pub fn set_room(room: Option<Arc<dyn Room>>) {
    if let Ok(mut slot) = slot().write() {
        *slot = room;
    }
}

pub fn room() -> Option<Arc<dyn Room>> {
    slot().read().ok().and_then(|slot| slot.clone())
}
