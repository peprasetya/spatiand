//! What a key sends to the program in the terminal: the bytes an xterm sends, which is what
//! every program in a terminal expects.

use spatiand_app::Key;

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
    }
}
