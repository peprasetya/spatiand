//! New passwords: twenty characters the room's keyboard can type.
//!
//! SpatiKey types an entry into a window with the room's US keyboard, and stops before a single
//! key is sent if a character is not on it (docs/spatikey.md, "Typing into a window"); so a
//! password made here is made only of what that keyboard types: its letters, its digits and the
//! symbols printed on its keys -- but not the space, which some sites trim off. At least one of
//! each kind is in it, since many sites still insist on that.

use ring::rand::{SecureRandom, SystemRandom};
use zeroize::Zeroizing;

/// Every character the room's US keyboard types but the space: every printable ASCII
/// character (`us_key` in crates/spatiand/src/backend_snapshot.rs has the keys for them).
pub const TYPEABLE: &str = concat!(
    "abcdefghijklmnopqrstuvwxyz",
    "ABCDEFGHIJKLMNOPQRSTUVWXYZ",
    "0123456789",
    "!@#$%^&*()-_=+[]{};:'\",.<>/?\\|`~",
);

pub const LENGTH: usize = 20;

/// A new password of [`LENGTH`] characters. Fails only if the system will not give random
/// numbers.
pub fn password() -> Result<Zeroizing<String>, String> {
    let random = SystemRandom::new();
    // A draw missing a kind of character is drawn again (about one in nine are), which keeps
    // every password that has all four kinds as likely as every other.
    for _ in 0..64 {
        let drawn = draw(LENGTH, |bytes| random.fill(bytes).is_ok()).ok_or("the system gave no random numbers")?;
        if every_kind(&drawn) {
            return Ok(drawn);
        }
    }
    Err("no password with every kind of character came of 64 tries".into())
}

/// `length` characters of [`TYPEABLE`], each chosen evenly from the random bytes `fill` gives.
/// A byte past the last whole round of the alphabet is thrown away rather than folded back,
/// which would make the first characters a little likelier than the rest.
fn draw(length: usize, mut fill: impl FnMut(&mut [u8]) -> bool) -> Option<Zeroizing<String>> {
    let alphabet = TYPEABLE.as_bytes();
    let limit = 256 - 256 % alphabet.len();
    let mut drawn = Zeroizing::new(String::with_capacity(length));
    let mut bytes = Zeroizing::new([0u8; 64]);
    while drawn.len() < length {
        if !fill(&mut bytes[..]) {
            return None;
        }
        for &byte in bytes.iter() {
            if (byte as usize) < limit && drawn.len() < length {
                drawn.push(alphabet[byte as usize % alphabet.len()] as char);
            }
        }
    }
    Some(drawn)
}

fn every_kind(password: &str) -> bool {
    password.chars().any(|c| c.is_ascii_lowercase())
        && password.chars().any(|c| c.is_ascii_uppercase())
        && password.chars().any(|c| c.is_ascii_digit())
        && password.chars().any(|c| c.is_ascii_punctuation())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn twenty_characters_the_room_can_type_of_every_kind() {
        for _ in 0..500 {
            let made = password().unwrap();
            assert_eq!(made.chars().count(), LENGTH);
            assert!(made.chars().all(|c| TYPEABLE.contains(c)));
            assert!(every_kind(&made));
        }
    }

    #[test]
    fn the_alphabet_is_the_keyboard_but_the_space() {
        let chars: std::collections::HashSet<char> = TYPEABLE.chars().collect();
        assert_eq!(chars.len(), TYPEABLE.len(), "nothing twice");
        // Every printable ASCII character but the space, and nothing else.
        assert_eq!(chars.len(), 94);
        assert!((b'!'..=b'~').all(|b| chars.contains(&(b as char))));
    }

    #[test]
    fn bytes_past_the_last_whole_round_are_not_used() {
        // With 94 characters, 188 and above would favour the first 68: they are skipped.
        let mut feed = vec![250u8, 188, 0, 93, 187, 94].into_iter();
        let made = draw(4, |bytes| {
            for b in bytes.iter_mut() {
                *b = feed.next().unwrap_or(255);
            }
            true
        })
        .unwrap();
        assert_eq!(made.as_str(), "a~~a");
        // A source that gives nothing gives no password.
        assert!(draw(4, |_| false).is_none());
    }

    #[test]
    fn two_are_not_the_same() {
        assert_ne!(*password().unwrap(), *password().unwrap());
    }
}
