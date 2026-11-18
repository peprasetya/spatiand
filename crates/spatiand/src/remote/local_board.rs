//! A clipboard of this machine's own, joined to the session's as a host's is: what the Mac's
//! `ClipboardSync` (mac/Sources/Spatiand/Clipboard.swift) does there, here for any platform whose
//! app can only read and write its system's clipboard -- the Beam Pro's (`spatiand-android`'s
//! `clipboard`).
//!
//! The platform says what its clipboard now holds, copied there by one of its own apps; this
//! turns that into what a host would say ([`Board::copied_text`], [`Board::copied_png`]), and
//! what the session says back into what to put on that clipboard and what to answer
//! ([`Board::said`]). The same rules as the Mac's:
//!
//! * text travels with the offer up to [`EAGER_TEXT`]; longer is sent when someone pastes;
//! * a picture (PNG) is offered by its size and sent when asked for; one offered by the session
//!   is fetched at once up to [`EAGER_IMAGE`], since a phone's clipboard cannot wait on a paste;
//! * what the session put there is not offered back to it, or two ends would pass one copy back
//!   and forth for ever;
//! * text the session marks secret -- offered with [`SECRET_HINT`], as a password manager
//!   copies it -- is put there as secret ([`Put::Secret`]: on Android, marked sensitive, so the
//!   phone does not show it in its clipboard preview). What the platform marks secret on its
//!   own clipboard is not offered to the session at all: the platform's side does not say it
//!   (`PhoneClipboard.java`, the Mac's `Clipboard.swift`).

use spatiand_stream::control::Clipboard;

/// Text up to this many bytes goes with the offer; the Mac's `eagerText`.
pub const EAGER_TEXT: usize = 256 * 1024;
/// A picture up to this many bytes is fetched as soon as it is offered; the Mac's `eagerImage`.
pub const EAGER_IMAGE: usize = 8 << 20;

const TEXT: &str = "text/plain;charset=utf-8";
const PNG: &str = "image/png";
/// The form a Wayland source offers beside its text to say the text is a secret (its bytes say
/// "secret"): KDE's convention, which password managers follow.
pub const SECRET_HINT: &str = "x-kde-passwordManagerHint";

/// What to put on the machine's own clipboard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Put {
    Text(String),
    /// Text the session marked secret (a password): put there marked so.
    Secret(String),
    Png(Vec<u8>),
}

/// The machine's side of the exchange: what it offered last, and what the session put on it.
#[derive(Debug, Default)]
pub struct Board {
    offered_text: Option<String>,
    offered_png: Option<Vec<u8>>,
    /// What the session last put on the machine's clipboard, which is not offered back to it.
    put: Option<Put>,
    /// Whether what the session offered last is secret, for its text sent later.
    secret: bool,
}

impl Board {
    /// The machine's clipboard holds this text now: what to say to the session, if anything.
    pub fn copied_text(&mut self, text: String) -> Option<Clipboard> {
        if text.is_empty() || self.put_text() == Some(text.as_str()) {
            return None;
        }
        let bytes = text.len();
        let eager = (bytes <= EAGER_TEXT).then(|| text.clone());
        self.offered_text = Some(text);
        self.offered_png = None;
        self.put = None;
        Some(Clipboard::Offer {
            mime_types: vec![TEXT.into()],
            text: eager,
            bytes: bytes.min(u32::MAX as usize) as u32,
        })
    }

    /// The machine's clipboard holds this picture now, as PNG: what to say to the session.
    pub fn copied_png(&mut self, png: Vec<u8>) -> Option<Clipboard> {
        if png.is_empty() || self.put.as_ref() == Some(&Put::Png(png.clone())) {
            return None;
        }
        let bytes = png.len().min(u32::MAX as usize) as u32;
        self.offered_png = Some(png);
        self.offered_text = None;
        self.put = None;
        Some(Clipboard::Offer {
            mime_types: vec![PNG.into()],
            text: None,
            bytes,
        })
    }

    /// The session said this: what to put on the machine's clipboard, and what to answer.
    pub fn said(&mut self, what: Clipboard) -> (Option<Put>, Option<Clipboard>) {
        match what {
            Clipboard::Offer { mime_types, text, bytes } => {
                self.secret = mime_types.iter().any(|t| t.eq_ignore_ascii_case(SECRET_HINT));
                if let Some(text) = text {
                    // Already there: written again, it would only come back as a new copy.
                    if self.offered_text.as_deref() == Some(text.as_str()) || self.put_text() == Some(text.as_str()) {
                        return (None, None);
                    }
                    let put = self.text_put(text);
                    return (Some(self.putting(put)), None);
                }
                let has = |m: &str| mime_types.iter().any(|t| t.eq_ignore_ascii_case(m));
                if has(PNG) && bytes as usize <= EAGER_IMAGE {
                    return (None, Some(Clipboard::Want { mime_type: PNG.into() }));
                }
                // Text too long to come with its offer: fetched now, as the picture is.
                if mime_types.iter().any(|t| is_text(t)) && bytes as usize <= EAGER_IMAGE {
                    return (None, Some(Clipboard::Want { mime_type: TEXT.into() }));
                }
                (None, None)
            }
            Clipboard::Want { mime_type } => (None, self.answer(&mime_type)),
            Clipboard::Data { mime_type, bytes } => {
                if mime_type.eq_ignore_ascii_case(PNG) {
                    if self.offered_png.as_deref() == Some(bytes.as_slice()) {
                        return (None, None);
                    }
                    return (Some(self.putting(Put::Png(bytes))), None);
                }
                if is_text(&mime_type) {
                    let text = String::from_utf8_lossy(&bytes).into_owned();
                    if self.offered_text.as_deref() == Some(text.as_str()) {
                        return (None, None);
                    }
                    let put = self.text_put(text);
                    return (Some(self.putting(put)), None);
                }
                (None, None)
            }
        }
    }

    /// Text to put there, secret if the session's offer said so.
    fn text_put(&self, text: String) -> Put {
        if self.secret {
            Put::Secret(text)
        } else {
            Put::Text(text)
        }
    }

    /// The text the session last put there, secret or not.
    fn put_text(&self) -> Option<&str> {
        match &self.put {
            Some(Put::Text(text) | Put::Secret(text)) => Some(text),
            _ => None,
        }
    }

    fn putting(&mut self, put: Put) -> Put {
        self.offered_text = None;
        self.offered_png = None;
        self.put = Some(put.clone());
        put
    }

    /// The bytes of what was offered, in the form asked for.
    fn answer(&self, mime_type: &str) -> Option<Clipboard> {
        let bytes = if mime_type.eq_ignore_ascii_case(PNG) {
            self.offered_png.clone()?
        } else if is_text(mime_type) {
            self.offered_text.clone()?.into_bytes()
        } else {
            return None;
        };
        Some(Clipboard::Data { mime_type: mime_type.to_string(), bytes })
    }
}

fn is_text(mime_type: &str) -> bool {
    let m = mime_type.to_ascii_lowercase();
    m.starts_with("text/plain") || m == "text" || m == "string" || m == "utf8_string"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_text_copied_on_the_machine_travels_with_its_offer() {
        let mut board = Board::default();
        let said = board.copied_text("hello".into()).unwrap();
        assert_eq!(said, Clipboard::Offer { mime_types: vec![TEXT.into()], text: Some("hello".into()), bytes: 5 });
    }

    #[test]
    fn long_text_is_offered_by_its_size_and_sent_when_asked() {
        let mut board = Board::default();
        let long = "x".repeat(EAGER_TEXT + 1);
        let Some(Clipboard::Offer { text, bytes, .. }) = board.copied_text(long.clone()) else { panic!() };
        assert_eq!(text, None);
        assert_eq!(bytes as usize, long.len());
        let (put, answer) = board.said(Clipboard::Want { mime_type: TEXT.into() });
        assert_eq!(put, None);
        assert_eq!(answer, Some(Clipboard::Data { mime_type: TEXT.into(), bytes: long.into_bytes() }));
    }

    #[test]
    fn what_the_session_put_there_is_not_offered_back() {
        let mut board = Board::default();
        let (put, _) = board.said(Clipboard::Offer { mime_types: vec![TEXT.into()], text: Some("from a host".into()), bytes: 11 });
        assert_eq!(put, Some(Put::Text("from a host".into())));
        // The platform sees its clipboard change to what was just put there.
        assert_eq!(board.copied_text("from a host".into()), None);
        // And a new copy on the machine is offered again.
        assert!(board.copied_text("mine".into()).is_some());
    }

    #[test]
    fn an_offer_of_what_the_machine_itself_offered_is_not_written_back() {
        let mut board = Board::default();
        board.copied_text("same".into());
        assert_eq!(board.said(Clipboard::Offer { mime_types: vec![TEXT.into()], text: Some("same".into()), bytes: 4 }), (None, None));
    }

    #[test]
    fn a_picture_offered_by_the_session_is_fetched_and_put_on_the_machine() {
        let mut board = Board::default();
        let (put, answer) = board.said(Clipboard::Offer { mime_types: vec!["image/png".into()], text: None, bytes: 1000 });
        assert_eq!(put, None);
        assert_eq!(answer, Some(Clipboard::Want { mime_type: PNG.into() }));
        let (put, answer) = board.said(Clipboard::Data { mime_type: PNG.into(), bytes: vec![1, 2, 3] });
        assert_eq!(put, Some(Put::Png(vec![1, 2, 3])));
        assert_eq!(answer, None);
        assert_eq!(board.copied_png(vec![1, 2, 3]), None);
    }

    #[test]
    fn a_picture_too_big_to_fetch_unasked_is_left_where_it_is() {
        let mut board = Board::default();
        let offer = Clipboard::Offer { mime_types: vec![PNG.into()], text: None, bytes: (EAGER_IMAGE + 1) as u32 };
        assert_eq!(board.said(offer), (None, None));
    }

    #[test]
    fn a_picture_copied_on_the_machine_is_answered_when_wanted() {
        let mut board = Board::default();
        let Some(Clipboard::Offer { mime_types, text, bytes }) = board.copied_png(vec![9; 10]) else { panic!() };
        assert_eq!((mime_types, text, bytes), (vec![PNG.to_string()], None, 10));
        assert_eq!(board.said(Clipboard::Want { mime_type: PNG.into() }).1, Some(Clipboard::Data { mime_type: PNG.into(), bytes: vec![9; 10] }));
        // Text is not what it holds.
        assert_eq!(board.said(Clipboard::Want { mime_type: TEXT.into() }).1, None);
    }

    #[test]
    fn text_the_session_marks_secret_is_put_there_as_secret() {
        let mut board = Board::default();
        let secret = vec![TEXT.into(), SECRET_HINT.into()];
        let (put, _) = board.said(Clipboard::Offer { mime_types: secret.clone(), text: Some("hunter2".into()), bytes: 7 });
        assert_eq!(put, Some(Put::Secret("hunter2".into())));
        assert_eq!(board.copied_text("hunter2".into()), None, "not offered back");
        // Too long to come with its offer: secret when it comes.
        let (_, want) = board.said(Clipboard::Offer { mime_types: secret, text: None, bytes: (EAGER_TEXT + 1) as u32 });
        assert_eq!(want, Some(Clipboard::Want { mime_type: TEXT.into() }));
        let (put, _) = board.said(Clipboard::Data { mime_type: TEXT.into(), bytes: b"long".to_vec() });
        assert_eq!(put, Some(Put::Secret("long".into())));
        // The next plain copy is plain.
        let (put, _) = board.said(Clipboard::Offer { mime_types: vec![TEXT.into()], text: Some("plain".into()), bytes: 5 });
        assert_eq!(put, Some(Put::Text("plain".into())));
    }

    #[test]
    fn nothing_is_not_offered() {
        let mut board = Board::default();
        assert_eq!(board.copied_text(String::new()), None);
        assert_eq!(board.copied_png(Vec::new()), None);
    }
}
