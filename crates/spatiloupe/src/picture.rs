//! Pictures: read, turned the right way up, and told whether they are two eyes' worth.
//!
//! The `image` crate reads what it can -- JPEG, PNG, GIF, WebP, TIFF and the rest -- and
//! FFmpeg the remainder: HEIF from phones, AVIF, JPEG 2000. SVG is drawn by resvg at the size
//! it is to be shown. What comes back is plain RGBA in memory, made no bigger than the GPU
//! will take.

use std::io::Cursor;
use std::path::Path;
use std::time::Duration;

use image::{AnimationDecoder, DynamicImage, ImageDecoder, ImageReader, RgbaImage};

use crate::stereo::{self, Eyes};

/// The most pixels a side the GPU is asked to hold: what every GPU Spatiand runs on takes.
pub const LARGEST: u32 = 8192;

/// A picture as it is to be shown.
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// For a picture with two eyes in it, how they are packed, as far as can be told.
    pub eyes: Eyes,
    /// A second picture, the right eye: the second image of an MPO.
    pub right: Option<(u32, u32, Vec<u8>)>,
    /// The frames of a moving picture after the first, each with how long it is shown.
    pub frames: Vec<(Vec<u8>, Duration)>,
    /// How long the first frame is shown, for a moving picture.
    pub first: Duration,
    /// What the picture was before it was made to fit: its own size.
    pub original: (u32, u32),
    /// The camera's account of it, a line each: when, with what, how.
    pub about: Vec<(String, String)>,
}

impl Picture {
    fn of(image: RgbaImage, original: (u32, u32)) -> Picture {
        let (width, height) = image.dimensions();
        Picture { width, height, rgba: image.into_raw(), eyes: Eyes::Mono, right: None, frames: Vec::new(), first: Duration::ZERO, original, about: Vec::new() }
    }
}

/// What can be shown as a picture, by name.
pub const PICTURES: &[&str] = &[
    "jpg", "jpeg", "jpe", "jfif", "png", "apng", "gif", "webp", "bmp", "tif", "tiff", "ico", "tga", "pbm", "pgm", "ppm", "pnm", "qoi", "hdr", "exr", "heic", "heif", "hif", "avif", "jp2", "j2k", "jpf", "jpx", "j2c", "jxl", "mpo", "jps", "pns", "svg", "dds",
];

fn ending(path: &Path) -> String {
    path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase()
}

/// Smaller, if it is larger than `largest` either way, keeping its shape.
fn fit(image: DynamicImage, largest: u32) -> DynamicImage {
    if image.width() <= largest && image.height() <= largest {
        return image;
    }
    image.resize(largest, largest, image::imageops::FilterType::Triangle)
}

/// Read the picture at `path`, at most `largest` pixels a side: [`LARGEST`] to look at, a few
/// hundred for a tile.
pub fn read(path: &Path, largest: u32) -> Result<Picture, String> {
    // What FFmpeg reads, it reads from the file or the stream itself.
    if ["heic", "heif", "hif", "avif", "jp2", "j2k", "jpf", "jpx", "j2c", "jxl"].contains(&ending(path).as_str()) {
        let mut picture = by_ffmpeg(path, largest)?;
        picture.eyes = stereo::guess(path, picture.width, picture.height, &picture.rgba);
        return Ok(picture);
    }
    let bytes = crate::place::read(path)?;
    read_bytes(path, &bytes, largest)
}

/// The same, from what has already been read: a picture that came over the network.
pub fn read_bytes(path: &Path, bytes: &[u8], largest: u32) -> Result<Picture, String> {
    let kind = ending(path);
    let mut picture = match kind.as_str() {
        "svg" => drawn(bytes, largest)?,
        "heic" | "heif" | "hif" | "avif" | "jp2" | "j2k" | "jpf" | "jpx" | "j2c" | "jxl" => by_ffmpeg(path, largest)?,
        _ => match by_image(bytes, largest, kind == "gif" || kind == "png" || kind == "apng" || kind == "webp") {
            Ok(picture) => picture,
            // Named wrongly, or something the image crate does not know: FFmpeg reads most
            // things that are pictures at all.
            Err(e) => by_ffmpeg(path, largest).map_err(|_| e)?,
        },
    };
    picture.about = about(bytes);
    // A second image after the first: a stereo camera's MPO, and some phones' JPEGs.
    if matches!(kind.as_str(), "mpo" | "jpg" | "jpeg") {
        if let Some(second) = stereo::mpo_second(bytes) {
            if let Ok(right) = image::load_from_memory_with_format(second, image::ImageFormat::Jpeg) {
                let right = fit(right, largest).to_rgba8();
                // Only a second image the same shape as the first is the other eye; a phone's
                // depth map or a small preview is not.
                let same = (right.width() as i64 * picture.height as i64 - right.height() as i64 * picture.width as i64).abs()
                    < (picture.width as i64 * picture.height as i64) / 50;
                if same {
                    let (w, h) = right.dimensions();
                    picture.right = Some((w, h, right.into_raw()));
                    picture.eyes = Eyes::Pair;
                }
            }
        }
    }
    if picture.right.is_none() {
        picture.eyes = stereo::guess(path, picture.width, picture.height, &picture.rgba);
    }
    Ok(picture)
}

fn by_image(bytes: &[u8], largest: u32, may_move: bool) -> Result<Picture, String> {
    let reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format().map_err(|e| e.to_string())?;
    let format = reader.format();
    if may_move {
        if let Some(picture) = moving(bytes, format, largest) {
            return Ok(picture);
        }
    }
    let mut decoder = reader.into_decoder().map_err(|e| e.to_string())?;
    let orientation = decoder.orientation().ok();
    let mut image = DynamicImage::from_decoder(decoder).map_err(|e| e.to_string())?;
    if let Some(orientation) = orientation {
        image.apply_orientation(orientation);
    }
    let original = (image.width(), image.height());
    Ok(Picture::of(fit(image, largest).to_rgba8(), original))
}

/// A GIF, an animated PNG or WebP, as its frames: `None` for one that is still.
fn moving(bytes: &[u8], format: Option<image::ImageFormat>, largest: u32) -> Option<Picture> {
    use image::ImageFormat;
    let frames: Vec<image::Frame> = match format? {
        ImageFormat::Gif => image::codecs::gif::GifDecoder::new(Cursor::new(bytes)).ok()?.into_frames().take(2000).collect::<Result<_, _>>().ok()?,
        ImageFormat::Png => {
            let decoder = image::codecs::png::PngDecoder::new(Cursor::new(bytes)).ok()?;
            if !decoder.is_apng().ok()? {
                return None;
            }
            decoder.apng().ok()?.into_frames().take(2000).collect::<Result<_, _>>().ok()?
        }
        ImageFormat::WebP => {
            let decoder = image::codecs::webp::WebPDecoder::new(Cursor::new(bytes)).ok()?;
            if !decoder.has_animation() {
                return None;
            }
            decoder.into_frames().take(2000).collect::<Result<_, _>>().ok()?
        }
        _ => return None,
    };
    if frames.len() < 2 {
        return None;
    }
    let length = |frame: &image::Frame| {
        let (n, d) = frame.delay().numer_denom_ms();
        // Browsers' rule: a delay of almost nothing is a tenth of a second.
        let ms = if d == 0 { 100 } else { n / d };
        Duration::from_millis(if ms < 20 { 100 } else { ms as u64 })
    };
    let mut frames = frames.into_iter();
    let first = frames.next()?;
    let first_length = length(&first);
    let buffer = first.into_buffer();
    let original = buffer.dimensions();
    let shrink = |image: RgbaImage| fit(DynamicImage::ImageRgba8(image), largest).to_rgba8();
    let mut picture = Picture::of(shrink(buffer), original);
    picture.first = first_length;
    picture.frames = frames.map(|frame| (length(&frame), frame.into_buffer())).map(|(l, b)| (shrink(b).into_raw(), l)).collect();
    Some(picture)
}

fn by_ffmpeg(path: &Path, largest: u32) -> Result<Picture, String> {
    let still = spatiand_media::still::grab(&crate::place::source(path), largest).ok_or("not a picture this can read")?;
    let image = RgbaImage::from_raw(still.width, still.height, still.rgba).ok_or("a picture of the wrong size")?;
    let image = fit(DynamicImage::ImageRgba8(image), largest).to_rgba8();
    let original = (still.width, still.height);
    Ok(Picture::of(image, original))
}

/// An SVG, drawn so its longer side is `largest`, or its own size if that is smaller and it
/// has one worth keeping.
fn drawn(bytes: &[u8], largest: u32) -> Result<Picture, String> {
    let options = resvg::usvg::Options::default();
    let tree = resvg::usvg::Tree::from_data(bytes, &options).map_err(|e| e.to_string())?;
    let size = tree.size();
    let longest = size.width().max(size.height()).max(1.0);
    // Drawn generously: a drawing is as sharp as it is drawn, and it is drawn once.
    let scale = (largest.min(4096) as f32 / longest).max(0.01);
    let (w, h) = (((size.width() * scale).ceil() as u32).max(1), ((size.height() * scale).ceil() as u32).max(1));
    let mut pixmap = resvg::tiny_skia::Pixmap::new(w, h).ok_or("an empty drawing")?;
    resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    // tiny-skia's pixels are multiplied by alpha; everything else here is not.
    let mut rgba = pixmap.take();
    for px in rgba.chunks_exact_mut(4) {
        let a = px[3] as u32;
        if a > 0 && a < 255 {
            for c in &mut px[..3] {
                *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    let image = RgbaImage::from_raw(w, h, rgba).ok_or("a drawing of the wrong size")?;
    Ok(Picture::of(image, (size.width().round() as u32, size.height().round() as u32)))
}

/// What the camera wrote down, in the words a person would use.
fn about(bytes: &[u8]) -> Vec<(String, String)> {
    let Ok(exif) = exif::Reader::new().read_from_container(&mut Cursor::new(bytes)) else { return Vec::new() };
    let get = |tag: exif::Tag| exif.get_field(tag, exif::In::PRIMARY).map(|f| f.display_value().with_unit(&exif).to_string());
    let mut about = Vec::new();
    if let Some(when) = get(exif::Tag::DateTimeOriginal).or_else(|| get(exif::Tag::DateTime)) {
        about.push(("Taken".to_string(), when));
    }
    let camera = [get(exif::Tag::Make), get(exif::Tag::Model)].into_iter().flatten().map(|s| s.trim_matches('"').trim().to_string()).collect::<Vec<_>>();
    if !camera.is_empty() {
        // "Apple Apple iPhone" reads as a mistake: the maker once.
        let text = if camera.len() == 2 && camera[1].starts_with(&camera[0]) { camera[1].clone() } else { camera.join(" ") };
        about.push(("Camera".to_string(), text));
    }
    if let Some(lens) = get(exif::Tag::LensModel) {
        about.push(("Lens".to_string(), lens.trim_matches('"').to_string()));
    }
    let exposure: Vec<String> = [get(exif::Tag::FNumber), get(exif::Tag::ExposureTime), get(exif::Tag::PhotographicSensitivity).map(|iso| format!("ISO {iso}")), get(exif::Tag::FocalLength)]
        .into_iter()
        .flatten()
        .collect();
    if !exposure.is_empty() {
        about.push(("Exposure".to_string(), exposure.join("   ")));
    }
    about
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(width: u32, height: u32, pixel: impl Fn(u32, u32) -> [u8; 4]) -> Vec<u8> {
        let image = RgbaImage::from_fn(width, height, |x, y| image::Rgba(pixel(x, y)));
        let mut out = Vec::new();
        DynamicImage::ImageRgba8(image).write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
        out
    }

    #[test]
    fn a_picture_too_big_for_the_gpu_is_made_to_fit_and_keeps_its_shape() {
        let bytes = png(1000, 250, |_, _| [9, 9, 9, 255]);
        let picture = read_bytes(Path::new("wide.png"), &bytes, 400).unwrap();
        assert_eq!((picture.width, picture.height), (400, 100));
        assert_eq!(picture.original, (1000, 250));
    }

    #[test]
    fn a_drawing_is_drawn() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><rect width="20" height="10" fill="#ff0000"/></svg>"##;
        let picture = read_bytes(Path::new("a.svg"), svg, 200).unwrap();
        assert_eq!((picture.width, picture.height), (200, 100));
        assert_eq!(&picture.rgba[..4], &[255, 0, 0, 255]);
    }

    #[test]
    fn a_misnamed_picture_is_still_read() {
        let bytes = png(8, 8, |_, _| [1, 2, 3, 255]);
        let picture = read_bytes(Path::new("really-a-png.jpg"), &bytes, 100).unwrap();
        assert_eq!(&picture.rgba[..4], &[1, 2, 3, 255]);
    }
}
