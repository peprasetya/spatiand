//! Reading a picture's text, and translating it, on threads of their own: hemera asked with the
//! AI settings as they are when it begins, and the asking called off -- hemera's work with it --
//! when what asked is dropped (the wearer moved on, the film plays on).

use std::sync::mpsc::{channel, Receiver};
use std::time::Instant;

use spatiand_voice::hemera::ocr::{self, Cancel, Failure, Kind, Options};
use spatiand_voice::Hemera;

use super::text::ReadText;

/// A picture as it is sent to be read.
pub struct Picture {
    pub bytes: Vec<u8>,
    pub kind: Kind,
    /// Its text is known to be upright -- a screenshot, a film's frame, a page drawn -- so hemera
    /// need not find out which way it is turned.
    pub upright: bool,
}

impl Picture {
    /// A file as it is, if hemera reads its kind (PNG, JPEG, WebP, TIFF) and it is within what it
    /// reads: a photograph goes as its own file, never made again (and worse) from its pixels.
    /// Its places come back in its stored pixels: its EXIF orientation is not hemera's business.
    pub fn file(bytes: Vec<u8>) -> Option<Picture> {
        let kind = Kind::of(&bytes)?;
        (bytes.len() <= ocr::MOST_BYTES).then_some(Picture { bytes, kind, upright: false })
    }

    /// Pixels -- four bytes each, red first, top row first -- as a PNG: a screenshot, a film's
    /// frame, a page drawn, a picture in a kind hemera does not read. Made quickly rather than
    /// small: it goes once, to a machine near by.
    pub fn png(rgba: &[u8], width: u32, height: u32, upright: bool) -> Result<Picture, String> {
        if rgba.len() != width as usize * height as usize * 4 || width == 0 || height == 0 {
            return Err(format!("a picture of {width} by {height} has {} bytes", rgba.len()));
        }
        let mut bytes = Vec::with_capacity(rgba.len() / 2);
        {
            let mut encoder = png::Encoder::new(&mut bytes, width, height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.set_compression(png::Compression::Fast);
            let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
            writer.write_image_data(rgba).map_err(|e| e.to_string())?;
        }
        Ok(Picture { bytes, kind: Kind::Png, upright })
    }
}

/// The largest picture worth sending whole, as so many pixels across and down for one `size`:
/// hemera reads up to 50 million pixels, and nothing is made smaller that is within that.
pub fn within_limit(size: (u32, u32)) -> (u32, u32) {
    let pixels = size.0 as u64 * size.1 as u64;
    if pixels <= ocr::MOST_PIXELS || pixels == 0 {
        return size;
    }
    let by = (ocr::MOST_PIXELS as f64 / pixels as f64).sqrt() * 0.995;
    (((size.0 as f64 * by).floor() as u32).max(1), ((size.1 as f64 * by).floor() as u32).max(1))
}

/// What went wrong, as it is said at a foot: a full sentence.
fn said(failure: &Failure) -> String {
    format!("The text could not be read: {}.", failure.words())
}

/// A picture's text being read.
pub struct Reading {
    cancel: Cancel,
    told: Receiver<Result<ReadText, String>>,
    started: Instant,
}

impl Reading {
    /// Read the picture `make` makes -- on the reading's own thread, since making a PNG of a 4K
    /// frame is not for the thread that draws -- with the wearer's languages and reading model.
    pub fn start(make: impl FnOnce() -> Result<Picture, String> + Send + 'static) -> Reading {
        let cancel = Cancel::new();
        let (tell, told) = channel();
        let called = cancel.clone();
        let _ = std::thread::Builder::new().name("live text".into()).spawn(move || {
            let picture = match make() {
                Ok(picture) => picture,
                Err(e) => {
                    let _ = tell.send(Err(format!("The text could not be read: {}.", e.trim_end_matches('.'))));
                    return;
                }
            };
            if called.cancelled() {
                return;
            }
            let ai = crate::ai::settings();
            let options = Options {
                languages: Options::languages(&ai.text_in_pictures.first_language, &ai.text_in_pictures.second_language),
                orientation: picture.upright.then_some(0),
                model: Some(ai.models.reading.clone()).filter(|m| !m.trim().is_empty()),
                ..Default::default()
            };
            let read = Hemera::new(&ai.hemera).read_text(&picture.bytes, picture.kind, &options, &called);
            let answer = match read {
                Ok(answer) => Ok(ReadText::of(&answer)),
                Err(Failure::Cancelled) => return,
                Err(failure) => Err(said(&failure)),
            };
            let _ = tell.send(answer);
        });
        Reading { cancel, told, started: Instant::now() }
    }

    /// What came of it, once it has come.
    pub fn poll(&self) -> Option<Result<ReadText, String>> {
        self.told.try_recv().ok()
    }

    pub fn started(&self) -> Instant {
        self.started
    }
}

impl Drop for Reading {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// Text being translated, by the AI settings' rule and everyday model: what comes is the language
/// it went into (two letters) and the translation.
pub struct Translating {
    cancel: Cancel,
    told: Receiver<Result<(String, String), String>>,
}

impl Translating {
    pub fn start(text: String) -> Translating {
        let cancel = Cancel::new();
        let (tell, told) = channel();
        let called = cancel.clone();
        let _ = std::thread::Builder::new().name("translation".into()).spawn(move || {
            let ai = crate::ai::settings();
            let pictures = &ai.text_in_pictures;
            let done = Hemera::new(&ai.hemera).translate(&ai.models.everyday, &text, &pictures.first_language, &pictures.second_language, &called);
            let answer = match done {
                Ok(done) => Ok(done),
                Err(Failure::Cancelled) => return,
                Err(failure) => Err(format!("It could not be translated: {}.", failure.words())),
            };
            let _ = tell.send(answer);
        });
        Translating { cancel, told }
    }

    pub fn poll(&self) -> Option<Result<(String, String), String>> {
        self.told.try_recv().ok()
    }
}

impl Drop for Translating {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picture_is_sent_whole_unless_hemera_would_refuse_it() {
        assert_eq!(within_limit((3840, 2160)), (3840, 2160));
        assert!(within_limit((8192, 8192)).0 as u64 * within_limit((8192, 8192)).1 as u64 <= ocr::MOST_PIXELS);
        let (w, h) = within_limit((12000, 6000));
        assert!((w as f32 / h as f32 - 2.0).abs() < 0.01, "its shape kept");
        assert_eq!(within_limit((0, 0)), (0, 0));
    }

    #[test]
    fn pixels_go_as_a_png_that_reads_back_the_same() {
        let rgba: Vec<u8> = (0..32 * 8 * 4).map(|i| (i * 7 % 251) as u8).collect();
        let picture = Picture::png(&rgba, 32, 8, true).unwrap();
        assert_eq!(picture.kind, Kind::Png);
        assert!(picture.upright);
        let decoder = png::Decoder::new(std::io::Cursor::new(&picture.bytes));
        let mut reader = decoder.read_info().unwrap();
        let mut back = vec![0; reader.output_buffer_size().unwrap()];
        reader.next_frame(&mut back).unwrap();
        assert_eq!(back, rgba);
        assert!(Picture::png(&rgba, 33, 8, false).is_err());
        // A file of a kind hemera reads goes as it is; another does not.
        assert!(Picture::file(picture.bytes.clone()).is_some_and(|p| p.kind == Kind::Png && !p.upright));
        assert!(Picture::file(b"GIF89a....".to_vec()).is_none());
    }
}
