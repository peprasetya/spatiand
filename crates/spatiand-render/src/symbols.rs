//! The room's own symbols: drawn here, not looked up in a font.
//!
//! Four characters the room sets as controls and marks are emoji, and an emoji is a different
//! thing on every platform. The microphone on the keyboard's dictation key, the speaker and
//! the silenced speaker on its sound toggle (and before a playing window's title), and the
//! battery's charging bolt. Measured through this renderer on 2026-11-09, by
//! `cargo test -p spatiand-render` on the Mac and by the same renderer cross-built for the
//! Beam Pro:
//!
//! - the Deck's font stack has all four in monochrome, and they took the ink colour;
//! - a Mac drew both speakers as Apple Color Emoji, coloured pictures that ignore the ink,
//!   and the microphone as a grey one;
//! - the Beam Pro drew **nothing**. Its only fonts with these characters are
//!   `NotoColorEmoji.ttf`, a COLRv1 font whose pictures swash cannot paint, and its bitmap
//!   twin. So the dictation key was a blank cap (the owner's report) and so was the toggle.
//!
//! Drawn from one outline each, they are the same picture in the text's own colour on every
//! platform, set and sized like the glyphs around them. Plain shapes, not anyone's icon set.
//!
//! [`crate::TextRenderer`] does the substitution: every string it renders gets these, so a
//! caller just writes the character.

use resvg::{tiny_skia, usvg};

/// One of the symbols this module draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Symbol {
    /// U+1F3A4 MICROPHONE: the keyboard's dictation key.
    Mic,
    /// U+1F50A SPEAKER WITH THREE SOUND WAVES: the click is on; a window is playing.
    Speaker,
    /// U+1F507 SPEAKER WITH CANCELLATION STROKE: the click is off.
    Silenced,
    /// U+26A1 HIGH VOLTAGE SIGN: the battery is charging.
    Charging,
}

impl Symbol {
    /// Every symbol, for tests and for anyone listing what is drawn.
    pub const ALL: [Symbol; 4] = [Symbol::Mic, Symbol::Speaker, Symbol::Silenced, Symbol::Charging];

    /// The symbol a character stands for, if it is one of these.
    pub fn of(c: char) -> Option<Symbol> {
        match c {
            '\u{1F3A4}' => Some(Symbol::Mic),
            '\u{1F50A}' => Some(Symbol::Speaker),
            '\u{1F507}' => Some(Symbol::Silenced),
            '\u{26A1}' => Some(Symbol::Charging),
            _ => None,
        }
    }

    /// The character a caller writes for it.
    pub fn char(self) -> char {
        match self {
            Symbol::Mic => '\u{1F3A4}',
            Symbol::Speaker => '\u{1F50A}',
            Symbol::Silenced => '\u{1F507}',
            Symbol::Charging => '\u{26A1}',
        }
    }

    /// Its outline, on a 24-unit em square, in white: the coverage is all that is used.
    ///
    /// Solid bodies with strokes of 2.2 units, because these are read at a degree or two
    /// across in the glasses, where a hairline is the first thing to go.
    fn svg(self) -> &'static str {
        match self {
            Symbol::Mic => concat!(
                r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24">"##,
                r##"<rect x="8.5" y="2" width="7" height="12.5" rx="3.5" fill="#fff"/>"##,
                r##"<path d="M5 11.5a7 7 0 0 0 14 0M12 18.5v3M8.5 21.5h7" fill="none" "##,
                r##"stroke="#fff" stroke-width="2.2" stroke-linecap="round"/></svg>"##,
            ),
            Symbol::Speaker => concat!(
                r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24">"##,
                r##"<path d="M2.5 9.2h3.8l5.2-4.4v14.4l-5.2-4.4H2.5z" fill="#fff" stroke="#fff" "##,
                r##"stroke-width="1.2" stroke-linejoin="round"/>"##,
                r##"<path d="M15.2 8.8a4.5 4.5 0 0 1 0 6.4M17.7 6.3a8 8 0 0 1 0 11.4" fill="none" "##,
                r##"stroke="#fff" stroke-width="2.2" stroke-linecap="round"/></svg>"##,
            ),
            Symbol::Silenced => concat!(
                r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24">"##,
                r##"<path d="M2.5 9.2h3.8l5.2-4.4v14.4l-5.2-4.4H2.5z" fill="#fff" stroke="#fff" "##,
                r##"stroke-width="1.2" stroke-linejoin="round"/>"##,
                r##"<path d="M15 9l6 6M21 9l-6 6" fill="none" stroke="#fff" stroke-width="2.2" "##,
                r##"stroke-linecap="round"/></svg>"##,
            ),
            Symbol::Charging => concat!(
                r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24">"##,
                r##"<path d="M13.5 2L4.5 13.5H11L9.5 22l10-12H13z" fill="#fff" stroke="#fff" "##,
                r##"stroke-width="1" stroke-linejoin="round"/></svg>"##,
            ),
        }
    }

    /// The symbol rasterised into a square `size` pixels across: one coverage byte per
    /// pixel, row by row. `None` only if the outline could not be drawn at all.
    pub fn coverage(self, size: u32) -> Option<Vec<u8>> {
        let size = size.max(1);
        let tree = usvg::Tree::from_str(self.svg(), &usvg::Options::default()).ok()?;
        let mut pixmap = tiny_skia::Pixmap::new(size, size)?;
        let scale = size as f32 / 24.0;
        resvg::render(
            &tree,
            tiny_skia::Transform::from_scale(scale, scale),
            &mut pixmap.as_mut(),
        );
        // Drawn in white, so the premultiplied alpha is the coverage and nothing else is
        // needed from the pixel.
        Some(pixmap.pixels().iter().map(|p| p.alpha()).collect())
    }
}

/// What a symbol is shaped as: a character every font has, about an em wide, and neutral to
/// the bidirectional algorithm, as an emoji is. A letter would make a right-to-left title
/// start left-to-right, and a space would be trimmed at a line's end, or wrapped. Its own
/// glyph is never drawn: the symbol is painted where it was set.
pub const STAND_IN: char = '@';

/// Where the middle of a symbol sits, in ems above the baseline: halfway up the capitals, so a
/// speaker before a title sits level with its first letter. Roboto's capitals are 0.71 em,
/// SF's 0.70, and DejaVu's 0.73.
pub const MIDDLE: f32 = 0.355;

/// `text` with each symbol replaced by [`STAND_IN`], and each one's byte offset in what is
/// returned. `None` when there is nothing to replace, which is nearly always.
///
/// A variation selector after a symbol goes with it: it asked for the emoji or the text
/// form of a character that is now neither.
pub fn stand_ins(text: &str) -> Option<(String, Vec<(usize, Symbol)>)> {
    if !text.chars().any(|c| Symbol::of(c).is_some()) {
        return None;
    }
    let mut shaped = String::with_capacity(text.len());
    let mut marks = Vec::new();
    let mut after_symbol = false;
    for c in text.chars() {
        if after_symbol && matches!(c, '\u{FE0E}' | '\u{FE0F}') {
            continue;
        }
        match Symbol::of(c) {
            Some(symbol) => {
                marks.push((shaped.len(), symbol));
                shaped.push(STAND_IN);
                after_symbol = true;
            }
            None => {
                shaped.push(c);
                after_symbol = false;
            }
        }
    }
    Some((shaped, marks))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_symbol_is_its_own_character_and_back() {
        for symbol in Symbol::ALL {
            assert_eq!(Symbol::of(symbol.char()), Some(symbol));
        }
        assert_eq!(Symbol::of('a'), None);
        assert_eq!(Symbol::of(STAND_IN), None);
    }

    #[test]
    fn every_symbol_draws_something_and_leaves_a_margin() {
        // Ink, and none of it touching the square's edge: a symbol that fills its em square
        // runs into its neighbours.
        for symbol in Symbol::ALL {
            let size = 48;
            let cov = symbol.coverage(size).expect("drawn");
            assert_eq!(cov.len(), (size * size) as usize);
            let inked = cov.iter().filter(|&&a| a > 128).count();
            assert!(inked > (size * size / 12) as usize, "{symbol:?} barely drew: {inked}");
            for i in 0..size as usize {
                for edge in [i, (size as usize - 1) * size as usize + i, i * size as usize, i * size as usize + size as usize - 1] {
                    assert!(cov[edge] < 64, "{symbol:?} reaches the edge of its square");
                }
            }
        }
    }

    #[test]
    fn the_symbols_are_different_pictures() {
        let drawn: Vec<Vec<u8>> = Symbol::ALL.iter().map(|s| s.coverage(32).unwrap()).collect();
        for i in 0..drawn.len() {
            for j in i + 1..drawn.len() {
                assert_ne!(drawn[i], drawn[j], "{:?} and {:?} look the same", Symbol::ALL[i], Symbol::ALL[j]);
            }
        }
    }

    #[test]
    fn stand_ins_replace_each_symbol_and_say_where() {
        assert!(stand_ins("plain text, no symbols").is_none());
        let (shaped, marks) = stand_ins("\u{1F50A} Songs \u{2014} YouTube").unwrap();
        assert_eq!(shaped, "@ Songs \u{2014} YouTube");
        assert_eq!(marks, vec![(0, Symbol::Speaker)]);
        let (shaped, marks) = stand_ins("80%\u{26A1}\u{FE0F} and \u{1F3A4}").unwrap();
        assert_eq!(shaped, "80%@ and @");
        assert_eq!(marks, vec![(3, Symbol::Charging), (9, Symbol::Mic)]);
        // A selector anywhere else is the text's own business.
        let (shaped, _) = stand_ins("\u{2764}\u{FE0F} \u{1F507}").unwrap();
        assert_eq!(shaped, "\u{2764}\u{FE0F} @");
    }
}
