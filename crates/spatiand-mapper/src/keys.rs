//! Keyboard keys a layout can press, by evdev code, with the names a picker shows.

/// In the order a picker lists them: the keys games use most first.
pub const KEYS: &[(u16, &str)] = &[
    (17, "W"),
    (30, "A"),
    (31, "S"),
    (32, "D"),
    (57, "Space"),
    (42, "Left Shift"),
    (29, "Left Ctrl"),
    (56, "Left Alt"),
    (15, "Tab"),
    (1, "Esc"),
    (28, "Enter"),
    (14, "Backspace"),
    (16, "Q"),
    (18, "E"),
    (19, "R"),
    (33, "F"),
    (34, "G"),
    (35, "H"),
    (44, "Z"),
    (45, "X"),
    (46, "C"),
    (47, "V"),
    (48, "B"),
    (20, "T"),
    (21, "Y"),
    (22, "U"),
    (23, "I"),
    (24, "O"),
    (25, "P"),
    (36, "J"),
    (37, "K"),
    (38, "L"),
    (49, "N"),
    (50, "M"),
    (2, "1"),
    (3, "2"),
    (4, "3"),
    (5, "4"),
    (6, "5"),
    (7, "6"),
    (8, "7"),
    (9, "8"),
    (10, "9"),
    (11, "0"),
    (103, "Up"),
    (108, "Down"),
    (105, "Left"),
    (106, "Right"),
    (59, "F1"),
    (60, "F2"),
    (61, "F3"),
    (62, "F4"),
    (63, "F5"),
    (64, "F6"),
    (65, "F7"),
    (66, "F8"),
    (67, "F9"),
    (68, "F10"),
    (87, "F11"),
    (88, "F12"),
    (54, "Right Shift"),
    (97, "Right Ctrl"),
    (100, "Right Alt"),
    (125, "Super"),
    (58, "Caps Lock"),
    (102, "Home"),
    (107, "End"),
    (104, "Page Up"),
    (109, "Page Down"),
    (110, "Insert"),
    (111, "Delete"),
    (12, "-"),
    (13, "="),
    (26, "["),
    (27, "]"),
    (39, ";"),
    (40, "'"),
    (41, "`"),
    (43, "\\"),
    (51, ","),
    (52, "."),
    (53, "/"),
    (82, "Keypad 0"),
    (79, "Keypad 1"),
    (80, "Keypad 2"),
    (81, "Keypad 3"),
    (75, "Keypad 4"),
    (76, "Keypad 5"),
    (77, "Keypad 6"),
    (71, "Keypad 7"),
    (72, "Keypad 8"),
    (73, "Keypad 9"),
    (96, "Keypad Enter"),
    (113, "Mute"),
    (114, "Volume down"),
    (115, "Volume up"),
    (164, "Play/Pause"),
    (163, "Next track"),
    (165, "Previous track"),
];

pub fn name(code: u16) -> Option<&'static str> {
    KEYS.iter().find(|(c, _)| *c == code).map(|(_, n)| *n)
}

/// The modifiers a [`Shortcut`] can hold, as `(bit, evdev code, name)`, in the order they go
/// down: Control, Shift, Alt, Super -- the left-hand key of each.
pub const MODIFIERS: [(u8, u16, &str); 4] = [(1, 29, "Ctrl"), (2, 42, "Shift"), (4, 56, "Alt"), (8, 125, "Super")];

/// Whether this key is a modifier, which goes down before the other keys of a frame and up
/// after them, so `Ctrl`+`T` pressed by one binding is Control-T and not a T then a Control.
pub fn is_modifier(code: u16) -> bool {
    matches!(code, 29 | 97 | 42 | 54 | 56 | 100 | 125 | 126)
}

/// A key with modifiers held, as one action: `ctrl+shift+t`. What a keyboard shortcut is.
///
/// Kept in a layout as the text a person writes (`keys = "ctrl+shift+t"`), and pressed as the
/// modifiers, then the key; let go of as the key, then the modifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Shortcut {
    /// [`MODIFIERS`]' bits.
    pub mods: u8,
    /// The key, by evdev code.
    pub key: u16,
}

impl Shortcut {
    pub const fn new(mods: u8, key: u16) -> Self {
        Self { mods, key }
    }

    /// Every key it holds, modifiers first.
    pub fn codes(self) -> impl Iterator<Item = u16> {
        MODIFIERS
            .into_iter()
            .filter(move |(bit, _, _)| self.mods & bit != 0)
            .map(|(_, code, _)| code)
            .chain(std::iter::once(self.key))
    }

    /// `Ctrl+Shift+T`, as a picker shows it.
    pub fn label(self) -> String {
        let mut parts: Vec<String> = MODIFIERS
            .iter()
            .filter(|(bit, _, _)| self.mods & bit != 0)
            .map(|(_, _, name)| name.to_string())
            .collect();
        parts.push(name(self.key).map(String::from).unwrap_or_else(|| self.key.to_string()));
        parts.join("+")
    }

    /// `ctrl+shift+t`, as a layout keeps it.
    pub fn text(self) -> String {
        self.label().to_lowercase()
    }

    /// From what a person writes: modifiers and a key joined by `+`, any case -- `ctrl+shift+t`,
    /// `Alt+Left`, `super+page up`, `ctrl+28` (a key may be its evdev number).
    pub fn parse(text: &str) -> Result<Shortcut, String> {
        let parts: Vec<&str> = text.split('+').map(str::trim).collect();
        let Some((last, mods)) = parts.split_last() else {
            return Err("an empty shortcut".into());
        };
        let mut bits = 0;
        for m in mods {
            bits |= modifier_bit(m).ok_or_else(|| format!("{m:?} is not Ctrl, Shift, Alt or Super"))?;
        }
        let key = key_code(last).ok_or_else(|| format!("{last:?} is not a key this knows"))?;
        Ok(Shortcut { mods: bits, key })
    }
}

fn modifier_bit(name: &str) -> Option<u8> {
    Some(match name.to_ascii_lowercase().as_str() {
        "ctrl" | "control" => 1,
        "shift" => 2,
        "alt" | "option" | "opt" => 4,
        "super" | "logo" | "meta" | "win" | "cmd" | "command" => 8,
        _ => return None,
    })
}

fn key_code(name: &str) -> Option<u16> {
    let lower = name.to_ascii_lowercase();
    // A digit is the key of that digit; a longer number is an evdev code.
    if lower.len() > 1 {
        if let Ok(code) = lower.parse::<u16>() {
            return Some(code);
        }
    }
    if let Some((code, _)) = KEYS.iter().find(|(_, n)| n.to_ascii_lowercase() == lower) {
        return Some(*code);
    }
    Some(match lower.as_str() {
        "ctrl" | "control" => 29,
        "shift" => 42,
        "alt" => 56,
        "super" | "meta" => 125,
        "escape" => 1,
        "return" => 28,
        "pgup" | "pageup" => 104,
        "pgdn" | "pagedown" => 109,
        "del" => 111,
        "ins" => 110,
        "spacebar" => 57,
        "minus" => 12,
        "equal" | "plus" => 13,
        _ => return None,
    })
}

impl serde::Serialize for Shortcut {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.text())
    }
}

impl<'de> serde::Deserialize<'de> for Shortcut {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        Shortcut::parse(&text).map_err(serde::de::Error::custom)
    }
}

/// Shortcuts a picker offers, the ones used most first. Any other is made from one of these by
/// changing its modifiers and its key, or written into the layout by hand.
pub const SHORTCUTS: [Shortcut; 24] = [
    Shortcut::new(1, 46),     // Ctrl+C
    Shortcut::new(1, 47),     // Ctrl+V
    Shortcut::new(1, 45),     // Ctrl+X
    Shortcut::new(1, 44),     // Ctrl+Z
    Shortcut::new(1 | 2, 44), // Ctrl+Shift+Z
    Shortcut::new(1, 30),     // Ctrl+A
    Shortcut::new(1, 33),     // Ctrl+F
    Shortcut::new(1, 31),     // Ctrl+S
    Shortcut::new(1, 20),     // Ctrl+T
    Shortcut::new(1 | 2, 20), // Ctrl+Shift+T
    Shortcut::new(1, 17),     // Ctrl+W
    Shortcut::new(1, 15),     // Ctrl+Tab
    Shortcut::new(1 | 2, 15), // Ctrl+Shift+Tab
    Shortcut::new(1, 38),     // Ctrl+L
    Shortcut::new(1, 19),     // Ctrl+R
    Shortcut::new(4, 105),    // Alt+Left
    Shortcut::new(4, 106),    // Alt+Right
    Shortcut::new(1, 13),     // Ctrl+=
    Shortcut::new(1, 12),     // Ctrl+-
    Shortcut::new(1, 11),     // Ctrl+0
    Shortcut::new(4, 62),     // Alt+F4
    Shortcut::new(0, 1),      // Esc
    Shortcut::new(0, 87),     // F11
    Shortcut::new(1 | 2, 49), // Ctrl+Shift+N
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_key_is_listed_twice() {
        let mut seen = std::collections::HashSet::new();
        for (code, name) in KEYS {
            assert!(seen.insert(*code), "{name} ({code}) listed twice");
        }
    }

    #[test]
    fn a_shortcut_is_written_as_a_person_writes_it_and_read_back() {
        let s = Shortcut::parse("Ctrl+Shift+T").unwrap();
        assert_eq!(s, Shortcut::new(1 | 2, 20));
        assert_eq!(s.text(), "ctrl+shift+t");
        assert_eq!(s.label(), "Ctrl+Shift+T");
        assert_eq!(s.codes().collect::<Vec<_>>(), vec![29, 42, 20]);
        assert_eq!(Shortcut::parse("alt+left").unwrap(), Shortcut::new(4, 105));
        assert_eq!(Shortcut::parse("super+page up").unwrap(), Shortcut::new(8, 104));
        assert_eq!(Shortcut::parse("ctrl+1").unwrap(), Shortcut::new(1, 2), "a digit is its key");
        assert_eq!(Shortcut::parse("ctrl+28").unwrap(), Shortcut::new(1, 28), "a number is a code");
        assert!(Shortcut::parse("hyper+t").is_err());
        assert!(Shortcut::parse("ctrl+nosuchkey").is_err());
        for s in SHORTCUTS {
            assert_eq!(Shortcut::parse(&s.text()), Ok(s), "{}", s.label());
        }
    }

    #[test]
    fn the_movement_keys_have_their_evdev_codes() {
        // A transposition here is a game that walks backwards.
        assert_eq!(name(17), Some("W"));
        assert_eq!(name(30), Some("A"));
        assert_eq!(name(31), Some("S"));
        assert_eq!(name(32), Some("D"));
    }
}
