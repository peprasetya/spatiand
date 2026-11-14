//! What a key sends to the program in the terminal: the bytes an xterm sends, which is what
//! every program in a terminal expects.

use spatiand_app::{Key, Modifiers};

/// The right Control key's evdev code. On a Mac the room tells either Command key to its
/// clients as this key (`crates/spatiand-mac/src/mac/ffi.rs`, `Command`), so a chord made with
/// it is a Mac's Command chord; elsewhere it is simply the right Control.
pub const RIGHT_CONTROL: u32 = 97;

/// What a key chord does with the clipboard (the owner, 2026-11-14: "use the cmd/ctrl c / v
/// for copy paste, instead of select for auto copy or right click for paste").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Clip {
    /// Copy what is selected, and leave it selected.
    Copy,
    /// Copy what is selected and clear it; with nothing selected, the key goes to the program
    /// (Control-C, the interrupt it has always been).
    CopyElseKey,
    /// Paste the clipboard.
    Paste,
    /// Not a clipboard chord: the key goes on as it would.
    Key,
}

/// The clipboard rule for a key. `right_control` is the right Control key held (Command on a
/// Mac); `alternate` is a full-screen program on the terminal's other screen.
///
/// * Control-Shift-C and -V, the logo key's C and V, and the right Control's (a Mac's
///   Command): always copy and paste -- never an interrupt, never a literal Control-V.
/// * The left Control's C: copies when something is selected (decided in the tab, which holds
///   the selection), the interrupt otherwise.
/// * The left Control's V: pastes at a shell's prompt and in whatever runs on the main screen;
///   a full-screen program (vim's block selection, nano's next page) gets the Control-V it asked
///   for, and is pasted into by Control-Shift-V.
/// * Alt held with any of them: the program's chord, as before.
pub fn clip(letter: Option<char>, m: Modifiers, right_control: bool, alternate: bool) -> Clip {
    let Some(letter @ ('c' | 'v')) = letter.map(|c| c.to_ascii_lowercase()) else { return Clip::Key };
    if m.alt {
        return Clip::Key;
    }
    let command = m.logo || (m.control && (m.shift || right_control));
    match (letter, command, m.control) {
        ('c', true, _) => Clip::Copy,
        ('v', true, _) => Clip::Paste,
        ('c', false, true) => Clip::CopyElseKey,
        ('v', false, true) if !alternate => Clip::Paste,
        _ => Clip::Key,
    }
}

/// What a paste sends the program: lines ended as Return ends them, and, for a program that
/// asked for bracketed paste (`TermMode::BRACKETED_PASTE`), between its brackets -- with any
/// Escape taken out, so pasted text cannot end the bracket early and be run as typed.
pub fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    let text = text.replace("\r\n", "\r").replace('\n', "\r");
    if bracketed {
        format!("\x1b[200~{}\x1b[201~", text.replace('\x1b', "")).into_bytes()
    } else {
        text.into_bytes()
    }
}

/// A key as a number, to cross from the window to the tab.
pub fn number(key: Key) -> u8 {
    match key {
        Key::Enter => 1,
        Key::Escape => 2,
        Key::Backspace => 3,
        Key::Delete => 4,
        Key::Tab => 5,
        Key::Space => 6,
        Key::Left => 7,
        Key::Right => 8,
        Key::Up => 9,
        Key::Down => 10,
        Key::Home => 11,
        Key::End => 12,
        Key::PageUp => 13,
        Key::PageDown => 14,
        Key::Char(_) => 15,
        Key::Other => 0,
    }
}

/// The modifier number xterm puts in a key's sequence: 1, plus 1 for Shift, 2 for Alt, 4
/// for Control.
fn modifier(shift: bool, alt: bool, control: bool) -> u8 {
    1 + shift as u8 + (alt as u8) * 2 + (control as u8) * 4
}

/// The bytes for a key. `app_cursor` is the program's asking for the cursor keys' other form
/// (`TermMode::APP_CURSOR`), which full-screen programs do.
pub fn bytes(key: u8, code: u32, text: &str, shift: bool, control: bool, alt: bool, app_cursor: bool) -> Vec<u8> {
    let m = modifier(shift, alt, control);
    let plain = m == 1;
    let csi = |end: char| -> Vec<u8> {
        if plain {
            if app_cursor { format!("\x1bO{end}") } else { format!("\x1b[{end}") }.into_bytes()
        } else {
            format!("\x1b[1;{m}{end}").into_bytes()
        }
    };
    let tilde = |n: u8| -> Vec<u8> {
        if plain { format!("\x1b[{n}~") } else { format!("\x1b[{n};{m}~") }.into_bytes()
    };
    let with_alt = |mut b: Vec<u8>| -> Vec<u8> {
        if alt {
            b.insert(0, 0x1b);
        }
        b
    };
    match key {
        1 => with_alt(b"\r".to_vec()),
        2 => b"\x1b".to_vec(),
        3 => with_alt(if control { b"\x08".to_vec() } else { b"\x7f".to_vec() }),
        4 => tilde(3),
        5 => {
            if shift {
                b"\x1b[Z".to_vec()
            } else {
                with_alt(b"\t".to_vec())
            }
        }
        6 => with_alt(if control { vec![0] } else { b" ".to_vec() }),
        7 => csi('D'),
        8 => csi('C'),
        9 => csi('A'),
        10 => csi('B'),
        11 => csi('H'),
        12 => csi('F'),
        13 => tilde(5),
        14 => tilde(6),
        _ => {
            // Keys the room's keys do not name, by where they are.
            match code {
                110 => return tilde(2),
                59..=62 => {
                    let end = (b'P' + (code - 59) as u8) as char;
                    return if plain { format!("\x1bO{end}").into_bytes() } else { format!("\x1b[1;{m}{end}").into_bytes() };
                }
                63..=68 => return tilde([15, 17, 18, 19, 20, 21][(code - 63) as usize]),
                87 => return tilde(23),
                88 => return tilde(24),
                _ => {}
            }
            if control {
                // Control and a letter is that letter's control character, whatever the
                // keyboard's own layout made of it.
                let c = text.chars().next().map(|c| c.to_ascii_lowercase());
                let control_char = match c {
                    Some(c @ 'a'..='z') => Some(c as u8 - b'a' + 1),
                    Some('@') | Some('2') | Some(' ') => Some(0),
                    Some('[') | Some('3') => Some(27),
                    Some('\\') | Some('4') => Some(28),
                    Some(']') | Some('5') => Some(29),
                    Some('^') | Some('6') => Some(30),
                    Some('_') | Some('-') | Some('/') | Some('7') => Some(31),
                    Some('8') | Some('?') => Some(127),
                    // The keyboard already made it one.
                    Some(c) if (c as u32) < 32 => Some(c as u8),
                    _ => None,
                };
                if let Some(b) = control_char {
                    return with_alt(vec![b]);
                }
            }
            // A key that types nothing (a modifier on its own) sends nothing, Alt or not.
            if text.is_empty() {
                return Vec::new();
            }
            with_alt(text.as_bytes().to_vec())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_send_what_an_xterm_sends() {
        assert_eq!(bytes(1, 28, "\r", false, false, false, false), b"\r");
        assert_eq!(bytes(9, 103, "", false, false, false, false), b"\x1b[A");
        assert_eq!(bytes(9, 103, "", false, false, false, true), b"\x1bOA");
        assert_eq!(bytes(8, 106, "", false, true, false, false), b"\x1b[1;5C");
        assert_eq!(bytes(15, 46, "c", false, true, false, false), [3]);
        assert_eq!(bytes(15, 46, "\x03", false, true, false, false), [3]);
        assert_eq!(bytes(15, 30, "a", false, false, true, false), b"\x1ba");
        assert_eq!(bytes(5, 15, "\t", true, false, false, false), b"\x1b[Z");
        assert_eq!(bytes(0, 60, "", false, false, false, false), b"\x1bOQ");
        assert_eq!(bytes(0, 88, "", false, false, false, false), b"\x1b[24~");
        assert_eq!(bytes(3, 14, "", false, false, false, false), [0x7f]);
        assert_eq!(bytes(15, 18, "é", false, false, false, false), "é".as_bytes());
        // Control, the right Control (a Mac's Command) and Alt pressed on their own send
        // nothing, so the tab leaves its selection be for the chord to come.
        assert!(bytes(0, 29, "", false, true, false, false).is_empty());
        assert!(bytes(0, RIGHT_CONTROL, "", false, true, false, false).is_empty());
        assert!(bytes(0, 56, "", false, false, true, false).is_empty());
    }

    fn held(shift: bool, control: bool, alt: bool, logo: bool) -> Modifiers {
        Modifiers { shift, control, alt, logo }
    }

    #[test]
    fn left_control_copies_a_selection_and_interrupts_without_one() {
        let ctrl = held(false, true, false, false);
        // Whether something is selected is the tab's to say: the chord is copy-or-interrupt.
        assert_eq!(clip(Some('c'), ctrl, false, false), Clip::CopyElseKey);
        assert_eq!(clip(Some('c'), ctrl, false, true), Clip::CopyElseKey);
        assert_eq!(clip(Some('C'), ctrl, false, false), Clip::CopyElseKey);
        // And when the tab has nothing selected, Control-C is the interrupt byte.
        assert_eq!(bytes(15, 46, "c", false, true, false, false), [3]);
    }

    #[test]
    fn left_control_v_pastes_except_in_a_full_screen_program() {
        let ctrl = held(false, true, false, false);
        assert_eq!(clip(Some('v'), ctrl, false, false), Clip::Paste);
        assert_eq!(clip(Some('v'), ctrl, false, true), Clip::Key);
        assert_eq!(bytes(15, 47, "v", false, true, false, false), [22]);
    }

    #[test]
    fn control_shift_always_copies_and_pastes() {
        let cs = held(true, true, false, false);
        for alternate in [false, true] {
            assert_eq!(clip(Some('c'), cs, false, alternate), Clip::Copy);
            assert_eq!(clip(Some('v'), cs, false, alternate), Clip::Paste);
            assert_eq!(clip(Some('c'), cs, true, alternate), Clip::Copy);
        }
    }

    #[test]
    fn the_right_control_is_a_macs_command_never_an_interrupt() {
        let ctrl = held(false, true, false, false);
        for alternate in [false, true] {
            assert_eq!(clip(Some('c'), ctrl, true, alternate), Clip::Copy);
            assert_eq!(clip(Some('v'), ctrl, true, alternate), Clip::Paste);
        }
        // Its other chords are the program's, as the left Control's are.
        assert_eq!(clip(Some('d'), ctrl, true, false), Clip::Key);
        assert_eq!(clip(Some('r'), ctrl, true, false), Clip::Key);
        // The logo key, where a keyboard's Super reaches the terminal, is the same.
        assert_eq!(clip(Some('c'), held(false, false, false, true), false, false), Clip::Copy);
    }

    #[test]
    fn what_is_no_clipboard_chord_goes_on() {
        assert_eq!(clip(Some('c'), Modifiers::default(), false, false), Clip::Key);
        assert_eq!(clip(Some('v'), held(true, false, false, false), false, false), Clip::Key);
        assert_eq!(clip(Some('c'), held(false, true, true, false), false, false), Clip::Key);
        assert_eq!(clip(Some('c'), held(false, true, true, false), true, false), Clip::Key);
        assert_eq!(clip(Some('x'), held(false, true, false, false), true, false), Clip::Key);
        assert_eq!(clip(None, held(false, true, false, false), true, false), Clip::Key);
        // The right Control let go of a moment ago is not held: a plain C is a C.
        assert_eq!(clip(Some('c'), Modifiers::default(), true, false), Clip::Key);
    }

    #[test]
    fn a_paste_is_bracketed_when_the_program_asked() {
        assert_eq!(paste_bytes("ls\npwd\r\n", false), b"ls\rpwd\r");
        assert_eq!(paste_bytes("ls\n", true), b"\x1b[200~ls\r\x1b[201~");
        // An Escape in the text cannot close the bracket early.
        assert_eq!(paste_bytes("a\x1b[201~rm", true), b"\x1b[200~a[201~rm\x1b[201~");
    }
}
