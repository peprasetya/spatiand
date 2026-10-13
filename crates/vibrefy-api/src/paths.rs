//! Addressing a file on the server.
//!
//! `CLIENT-API.md` names three traps here, and all three are silent failures rather than
//! errors, so this is worth its own module and its own tests:
//!
//!  1. a bare `?` truncates the path at the query string, so it must be `%3F`;
//!  2. `BeanObject.processRequest` runs the whole request URI through a form decoder,
//!     which reads `+` as a space - so spaces go as `%20` and a real plus as `%2B`;
//!  3. the `/` separators must be left alone: Jetty rejects `%2F` and a doubled `//` as
//!     an ambiguous path.
//!
//! `encodeURIComponent` per segment, joined with `/`, is the rule. This is the Rust of
//! `VibrefyClient.encodePath` in the Android client, and the two have to stay identical or
//! the same file gets two different progress keys.

/// Percent-encodes one path segment, leaving only the characters `encodeURIComponent`
/// leaves: unreserved plus `!'()*-._~`.
pub fn encode_segment(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len());
    for byte in segment.as_bytes() {
        let ch = *byte as char;
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '!' | '~' | '*' | '\'' | '(' | ')') {
            out.push(ch);
        } else {
            out.push('%');
            out.push_str(&format!("{byte:02X}"));
        }
    }
    out
}

/// Encodes each segment of a library-relative path and rejoins them. Empty segments are
/// dropped, which is what keeps a doubled slash from ever reaching Jetty.
pub fn encode_path(path: &str) -> String {
    path.split('/')
        .filter(|s| !s.is_empty())
        .map(encode_segment)
        .collect::<Vec<_>>()
        .join("/")
}

/// The other direction, for a name that arrived inside a URL.
///
/// `vibrefy play <url>` has nothing but the URL to go on, and the last segment of it is
/// where the 3D and VR markings live - which means it has to be read as a *name* and not as
/// a path segment. `Test%20360.mp4` is not a file called "Test 360": the marking `360` is
/// welded to the word before it, and the layout detector scans for whole tokens, so it
/// finds a flat file. Percent-decoding first is what makes the URL form behave like the
/// library form.
///
/// Anything that is not a valid escape is left exactly as it stands; a name with a stray
/// `%` in it is a name, not an error.
pub fn decode_segment(segment: &str) -> String {
    let bytes = segment.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = |b: u8| match b {
                b'0'..=b'9' => Some(b - b'0'),
                b'a'..=b'f' => Some(b - b'a' + 10),
                b'A'..=b'F' => Some(b - b'A' + 10),
                _ => None,
            };
            if let (Some(high), Some(low)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push(high * 16 + low);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    // Invalid UTF-8 in a percent escape is somebody else's filename, not a reason to fail.
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn separators_survive_and_segments_are_encoded() {
        assert_eq!(
            "MyLib/Show/Ep%201.mp4",
            encode_path("MyLib/Show/Ep 1.mp4")
        );
    }

    /// The three traps `CLIENT-API.md` names, in one name.
    #[test]
    fn the_named_traps_are_all_escaped() {
        assert_eq!(
            "MyLib/Ep%201%3F%20%26%20%3Codd%3E.mp4",
            encode_path("MyLib/Ep 1? & <odd>.mp4")
        );
    }

    /// A space must never go as `+`, because the server's form decoder would read a real
    /// plus the same way and the two files would collide.
    #[test]
    fn a_space_is_never_a_plus_and_a_plus_is_never_a_space() {
        assert_eq!("a%20b", encode_segment("a b"));
        assert_eq!("a%2Bb", encode_segment("a+b"));
    }

    /// A cloud mount lives behind `~`, which `encodeURIComponent` leaves alone - and so
    /// must this, or `/files/~gdrive/...` stops resolving.
    #[test]
    fn a_cloud_prefix_is_left_alone() {
        assert_eq!("~gdrive/Films", encode_path("~gdrive/Films"));
    }

    #[test]
    fn empty_segments_cannot_produce_a_doubled_slash() {
        assert_eq!("a/b", encode_path("/a//b/"));
        assert_eq!("", encode_path(""));
    }

    /// Non-ASCII goes as UTF-8 bytes; the server normalises the form itself.
    #[test]
    fn utf8_is_encoded_byte_by_byte() {
        assert_eq!("Amelie%CC%81", encode_segment("Amelie\u{0301}"));
        assert_eq!("%E6%98%A0%E7%94%BB", encode_segment("映画"));
    }
}

#[cfg(test)]
mod decode_tests {
    use super::decode_segment;

    /// The one that mattered: `Test%20360.mp4` detected as flat, because the layout
    /// scanner looks for `360` as a word and `%20360` is not one.
    #[test]
    fn an_escaped_space_becomes_a_space_so_the_markings_read_as_words() {
        assert_eq!("Test 360.mp4", decode_segment("Test%20360.mp4"));
        assert_eq!("A+B.mp4", decode_segment("A%2BB.mp4"));
    }

    #[test]
    fn a_name_with_no_escapes_comes_back_untouched() {
        assert_eq!("Plain Name.mp4", decode_segment("Plain Name.mp4"));
    }

    /// A stray percent is part of a filename often enough to be worth not choking on.
    #[test]
    fn something_that_is_not_an_escape_is_left_alone() {
        assert_eq!("50% off.mp4", decode_segment("50% off.mp4"));
        assert_eq!("100%.mp4", decode_segment("100%.mp4"));
        assert_eq!("%zz.mp4", decode_segment("%zz.mp4"));
    }

    #[test]
    fn a_round_trip_through_the_encoder_comes_back_the_same() {
        for name in ["A Film (2019).mp4", "Q? & A.mp4", "a+b=c.mp4", "café.mp4"] {
            assert_eq!(name, decode_segment(&super::encode_segment(name)));
        }
    }
}
