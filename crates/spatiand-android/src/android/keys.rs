//! Android's keys as the Linux key codes the compositor's keyboard takes.
//!
//! Two ways in: a key a hardware keyboard sent, by Android's `KeyEvent` code, and text Android's
//! own on-screen keyboard committed, character by character. Both come out as evdev codes -- the
//! numbers `spatiand_shell::keyboard` and `send_key_state` use -- with shift where the character
//! needs it on a US layout, which is the layout the compositor's keymap is.

/// An evdev code, from Android's `KeyEvent.KEYCODE_*`.
pub fn from_android(code: i32) -> Option<u32> {
    Some(match code {
        // Letters: KEYCODE_A is 29 ... KEYCODE_Z is 54, in the alphabet's order, while evdev
        // numbers them by where they sit on the keyboard.
        29..=54 => LETTERS[(code - 29) as usize],
        // Digits: KEYCODE_0 is 7 ... KEYCODE_9 is 16.
        7 => 11,
        8..=16 => (code - 8 + 2) as u32,
        62 => 57,  // SPACE
        66 => 28,  // ENTER
        67 => 14,  // DEL, which is backspace
        112 => 111, // FORWARD_DEL
        61 => 15,  // TAB
        111 => 1,  // ESCAPE
        19 => 103, // DPAD_UP
        20 => 108, // DPAD_DOWN
        21 => 105, // DPAD_LEFT
        22 => 106, // DPAD_RIGHT
        122 => 102, // MOVE_HOME
        123 => 107, // MOVE_END
        92 => 104, // PAGE_UP
        93 => 109, // PAGE_DOWN
        124 => 110, // INSERT
        59 => 42,  // SHIFT_LEFT
        60 => 54,  // SHIFT_RIGHT
        113 => 29, // CTRL_LEFT
        114 => 97, // CTRL_RIGHT
        57 => 56,  // ALT_LEFT
        58 => 100, // ALT_RIGHT
        117 => 125, // META_LEFT
        118 => 126, // META_RIGHT
        115 => 58, // CAPS_LOCK
        68 => 41,  // GRAVE
        69 => 12,  // MINUS
        70 => 13,  // EQUALS
        71 => 26,  // LEFT_BRACKET
        72 => 27,  // RIGHT_BRACKET
        73 => 43,  // BACKSLASH
        74 => 39,  // SEMICOLON
        75 => 40,  // APOSTROPHE
        55 => 51,  // COMMA
        56 => 52,  // PERIOD
        76 => 53,  // SLASH
        131..=140 => (code - 131 + 59) as u32, // F1..F10 are 59..68
        141 => 87, // F11
        142 => 88, // F12
        _ => return None,
    })
}

/// evdev codes of A to Z.
const LETTERS: [u32; 26] = [
    30, 48, 46, 32, 18, 33, 34, 35, 23, 36, 37, 38, 50, 49, 24, 25, 16, 19, 31, 20, 22, 47, 17, 45,
    21, 44,
];

/// The key and whether shift is held for it, for one character of committed text on a US layout.
pub fn from_char(c: char) -> Option<(u32, bool)> {
    Some(match c {
        'a'..='z' => (LETTERS[(c as u8 - b'a') as usize], false),
        'A'..='Z' => (LETTERS[(c as u8 - b'A') as usize], true),
        '1'..='9' => ((c as u8 - b'1') as u32 + 2, false),
        '0' => (11, false),
        ' ' => (57, false),
        '\n' => (28, false),
        '\t' => (15, false),
        '-' => (12, false),
        '_' => (12, true),
        '=' => (13, false),
        '+' => (13, true),
        '[' => (26, false),
        '{' => (26, true),
        ']' => (27, false),
        '}' => (27, true),
        '\\' => (43, false),
        '|' => (43, true),
        ';' => (39, false),
        ':' => (39, true),
        '\'' => (40, false),
        '"' => (40, true),
        '`' => (41, false),
        '~' => (41, true),
        ',' => (51, false),
        '<' => (51, true),
        '.' => (52, false),
        '>' => (52, true),
        '/' => (53, false),
        '?' => (53, true),
        '!' => (2, true),
        '@' => (3, true),
        '#' => (4, true),
        '$' => (5, true),
        '%' => (6, true),
        '^' => (7, true),
        '&' => (8, true),
        '*' => (9, true),
        '(' => (10, true),
        ')' => (11, true),
        _ => return None,
    })
}
