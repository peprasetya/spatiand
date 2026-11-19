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
    read_as(path, largest, true)
}

/// The same, a moving picture's first frame only: for a tile, which does not move, and which a
/// long GIF's every frame is not to be decoded for.
pub fn read_still(path: &Path, largest: u32) -> Result<Picture, String> {
    read_as(path, largest, false)
}

fn read_as(path: &Path, largest: u32, may_move: bool) -> Result<Picture, String> {
    // What FFmpeg reads, it reads from the file or the stream itself.
    if ["heic", "heif", "hif", "avif", "jp2", "j2k", "jpf", "jpx", "j2c", "jxl"].contains(&ending(path).as_str()) {
        let mut picture = by_ffmpeg(path, largest)?;
        picture.eyes = stereo::guess(path, picture.width, picture.height, &picture.rgba);
        // The camera's account, without reading the file whole: FFmpeg has read the picture by
        // itself and kept none of it, so it is read from the file on this machine, and from a
        // remote's by the pieces it is in -- the head, and where its Exif is -- each fetched
        // by a range, never the whole file a second time. JPEG 2000 and JPEG XL keep no account
        // that is read, so a remote's are not asked for it.
        match crate::place::pieces(path) {
            Some(mut pieces) => {
                if ["heic", "heif", "hif", "avif"].contains(&ending(path).as_str()) {
                    picture.about = about_from(&mut pieces);
                }
            }
            None => {
                if let Ok(file) = std::fs::File::open(path) {
                    picture.about = about_from(&mut std::io::BufReader::new(file));
                }
            }
        }
        return Ok(picture);
    }
    let bytes = crate::place::read(path)?;
    read_bytes_as(path, &bytes, largest, may_move)
}

/// A picture whole, at its own size, the right way up, as it is to be saved once marked up: with
/// what its file said of it -- the camera's account and the colours' profile -- where the image
/// crate reads them, and the format its bytes are in.
pub struct Whole {
    pub image: RgbaImage,
    pub exif: Option<Vec<u8>>,
    pub icc: Option<Vec<u8>>,
    pub format: Option<image::ImageFormat>,
}

/// Read the picture at `path` whole, as [`read`] reads it for the window but not made smaller:
/// what a saved picture is drawn on. A drawing (SVG) is drawn as large as it is shown.
pub fn read_whole(path: &Path) -> Result<Whole, String> {
    let kind = ending(path);
    let plain = |picture: Picture| -> Result<Whole, String> {
        let image = RgbaImage::from_raw(picture.width, picture.height, picture.rgba).ok_or("a picture of the wrong size")?;
        Ok(Whole { image, exif: None, icc: None, format: None })
    };
    // FFmpeg's whole: as wide as a picture can be, which it then is not made smaller than.
    let whole = i32::MAX as u32;
    if ["heic", "heif", "hif", "avif", "jp2", "j2k", "jpf", "jpx", "j2c", "jxl"].contains(&kind.as_str()) {
        return plain(by_ffmpeg(path, whole)?);
    }
    let bytes = crate::place::read(path)?;
    if kind == "svg" {
        return plain(drawn(&bytes, LARGEST, if crate::place::remote(path).is_some() { None } else { path.parent() })?);
    }
    let read = || -> Result<Whole, String> {
        let mut reader = ImageReader::new(Cursor::new(&bytes)).with_guessed_format().map_err(|e| e.to_string())?;
        // A format with no mark of its own at its start (TGA) is what its name says.
        if reader.format().is_none() {
            if let Some(named) = image::ImageFormat::from_extension(&kind) {
                reader.set_format(named);
            }
        }
        let format = reader.format();
        let mut decoder = reader.into_decoder().map_err(|e| e.to_string())?;
        if decoder.total_bytes() > STILL_BYTES {
            let (w, h) = decoder.dimensions();
            return Err(format!("it is {w} \u{d7} {h} pixels, too large to save"));
        }
        let icc = decoder.icc_profile().ok().flatten();
        let exif = decoder.exif_metadata().ok().flatten();
        let orientation = decoder.orientation().ok();
        let mut image = DynamicImage::from_decoder(decoder).map_err(|e| e.to_string())?;
        if let Some(orientation) = orientation {
            image.apply_orientation(orientation);
        }
        Ok(Whole { image: shown(image), exif, icc, format })
    };
    // Named wrongly, or not the image crate's: FFmpeg, as for the window.
    read().or_else(|e| by_ffmpeg(path, whole).map_err(|_| e).and_then(plain))
}

/// The same, from what has already been read: a picture that came over the network.
#[cfg(test)]
pub fn read_bytes(path: &Path, bytes: &[u8], largest: u32) -> Result<Picture, String> {
    read_bytes_as(path, bytes, largest, true)
}

fn read_bytes_as(path: &Path, bytes: &[u8], largest: u32, may_move: bool) -> Result<Picture, String> {
    let kind = ending(path);
    let mut picture = match kind.as_str() {
        // A drawing's pictures are its folder's, on this machine; a remote's has none.
        "svg" => drawn(bytes, largest, if crate::place::remote(path).is_some() { None } else { path.parent() })?,
        "heic" | "heif" | "hif" | "avif" | "jp2" | "j2k" | "jpf" | "jpx" | "j2c" | "jxl" => by_ffmpeg(path, largest)?,
        _ => match by_image(bytes, &kind, largest, may_move && (kind == "gif" || kind == "png" || kind == "apng" || kind == "webp")) {
            Ok(picture) => picture,
            // Named wrongly, or something the image crate does not know: FFmpeg reads most
            // things that are pictures at all.
            Err(e) => by_ffmpeg(path, largest).map_err(|_| e)?,
        },
    };
    picture.about = about(bytes);
    // A second image after the first: a stereo camera's MPO, and some phones' JPEGs.
    if matches!(kind.as_str(), "mpo" | "jpg" | "jpeg") {
        if let Some((second, what)) = stereo::mpo_second(bytes) {
            let size = ImageReader::with_format(Cursor::new(second), image::ImageFormat::Jpeg).into_dimensions().ok();
            if size.is_some_and(|size| other_eye(what, size, picture.original)) {
                if let Ok(right) = image::load_from_memory_with_format(second, image::ImageFormat::Jpeg) {
                    let right = fit(right, largest).to_rgba8();
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

/// A picture the image crate reads: by what its bytes begin with, or -- a format that has no mark
/// of its own at its start (TGA) -- by its name's `ending`.
fn by_image(bytes: &[u8], ending: &str, largest: u32, may_move: bool) -> Result<Picture, String> {
    let mut reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format().map_err(|e| e.to_string())?;
    if reader.format().is_none() {
        if let Some(named) = image::ImageFormat::from_extension(ending) {
            reader.set_format(named);
        }
    }
    let format = reader.format();
    if may_move {
        if let Some(picture) = moving(bytes, format, largest) {
            return Ok(picture);
        }
    }
    let mut decoder = reader.into_decoder().map_err(|e| e.to_string())?;
    // Decoded whole before it is made small: a canvas that claims to be enormous (a GIF may say
    // 65535 a side in a few bytes) is not given the memory it asks for.
    if decoder.total_bytes() > STILL_BYTES {
        let (w, h) = decoder.dimensions();
        return Err(format!("it is {w} \u{d7} {h} pixels, too large to show"));
    }
    let orientation = decoder.orientation().ok();
    let mut image = DynamicImage::from_decoder(decoder).map_err(|e| e.to_string())?;
    if let Some(orientation) = orientation {
        image.apply_orientation(orientation);
    }
    let original = (image.width(), image.height());
    Ok(Picture::of(shown(fit(image, largest)), original))
}

/// A picture as eight bits a channel of sRGB, as it is shown. A Radiance HDR or an OpenEXR's
/// floats are light itself, linear and as bright as they are: brought within white (Reinhard's
/// curve, white at the brightest that is not a speck -- nothing changes below 1 if that is 1)
/// and given sRGB's curve, rather than cut off at 1 and taken for sRGB already, which showed
/// them dark and flat with their highlights burnt out.
fn shown(image: DynamicImage) -> RgbaImage {
    if !matches!(image, DynamicImage::ImageRgb32F(_) | DynamicImage::ImageRgba32F(_)) {
        return image.to_rgba8();
    }
    let light = image.into_rgba32f();
    let mut peaks: Vec<f32> = light.pixels().step_by((light.len() / 4 / 65536).max(1)).map(|p| p.0[0].max(p.0[1]).max(p.0[2])).filter(|v| v.is_finite()).collect();
    peaks.sort_by(f32::total_cmp);
    let white = peaks.get(peaks.len().saturating_sub(1) * 995 / 1000).copied().unwrap_or(1.0).clamp(1.0, 64.0);
    let encode = |c: f32| {
        let c = if c.is_finite() { c.max(0.0) } else { 0.0 };
        let c = (c * (1.0 + c / (white * white)) / (1.0 + c)).min(1.0);
        let c = if c <= 0.003_130_8 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 };
        (c * 255.0).round() as u8
    };
    RgbaImage::from_fn(light.width(), light.height(), |x, y| {
        let p = light.get_pixel(x, y).0;
        image::Rgba([encode(p[0]), encode(p[1]), encode(p[2]), (p[3].clamp(0.0, 1.0) * 255.0).round() as u8])
    })
}

/// The most memory a still picture may take as it is decoded, before it is made to fit.
const STILL_BYTES: u64 = 1 << 30;

/// The largest canvas a moving picture may have: every frame is a whole canvas.
const MOVING_SIDE: u32 = 8192;

/// How much a moving picture's frames may hold in memory, all together, as they are shown: the
/// frames past this are not kept, and it plays as far as they go.
const MOVING_BYTES: usize = 512 << 20;

/// What a decoder of a moving picture may take: a canvas no larger than [`MOVING_SIDE`] a side,
/// and one frame's worth of memory at a time within the image crate's usual bound.
fn moving_limits() -> image::Limits {
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MOVING_SIDE);
    limits.max_image_height = Some(MOVING_SIDE);
    limits
}

/// A GIF, an animated PNG or WebP, as its frames: `None` for one that is still -- or that is not
/// to be played, its canvas absurd -- which is then read as a still, within its limits.
fn moving(bytes: &[u8], format: Option<image::ImageFormat>, largest: u32) -> Option<Picture> {
    use image::ImageFormat;
    let frames: image::Frames = match format? {
        ImageFormat::Gif => {
            let mut decoder = image::codecs::gif::GifDecoder::new(Cursor::new(bytes)).ok()?;
            decoder.set_limits(moving_limits()).ok()?;
            decoder.into_frames()
        }
        ImageFormat::Png => {
            let mut decoder = image::codecs::png::PngDecoder::new(Cursor::new(bytes)).ok()?;
            if !decoder.is_apng().ok()? {
                return None;
            }
            decoder.set_limits(moving_limits()).ok()?;
            decoder.apng().ok()?.into_frames()
        }
        ImageFormat::WebP => {
            let mut decoder = image::codecs::webp::WebPDecoder::new(Cursor::new(bytes)).ok()?;
            if !decoder.has_animation() {
                return None;
            }
            decoder.set_limits(moving_limits()).ok()?;
            decoder.into_frames()
        }
        _ => return None,
    };
    let length = |frame: &image::Frame| {
        let (n, d) = frame.delay().numer_denom_ms();
        // Browsers' rule: a delay of almost nothing is a tenth of a second.
        let ms = if d == 0 { 100 } else { n / d };
        Duration::from_millis(if ms < 20 { 100 } else { ms as u64 })
    };
    let shrink = |image: RgbaImage| fit(DynamicImage::ImageRgba8(image), largest).to_rgba8();
    // Each frame made small as it comes, so that only one is ever at the canvas's own size.
    let mut picture: Option<Picture> = None;
    let mut held = 0usize;
    for frame in frames.take(2000) {
        let Ok(frame) = frame else { break };
        let frame_length = length(&frame);
        let buffer = frame.into_buffer();
        let original = buffer.dimensions();
        let small = shrink(buffer);
        held += small.as_raw().len();
        if held > MOVING_BYTES {
            break;
        }
        match picture.as_mut() {
            None => {
                let mut first = Picture::of(small, original);
                first.first = frame_length;
                picture = Some(first);
            }
            Some(picture) => picture.frames.push((small.into_raw(), frame_length)),
        }
    }
    picture.filter(|p| !p.frames.is_empty())
}

fn by_ffmpeg(path: &Path, largest: u32) -> Result<Picture, String> {
    let still = match crate::place::remote(path) {
        Some(_) => spatiand_media::still::grab(&crate::place::source(path), largest),
        None => spatiand_media::still::grab_file(path, largest),
    };
    let still = still.ok_or("not a picture this can read")?;
    let image = RgbaImage::from_raw(still.width, still.height, still.rgba).ok_or("a picture of the wrong size")?;
    let image = fit(DynamicImage::ImageRgba8(image), largest).to_rgba8();
    let original = (still.width, still.height);
    Ok(Picture::of(image, original))
}

/// The machine's fonts, for a drawing's text: read once, the first time a drawing wants them, and
/// shared by every drawing after. Android's are in /system/fonts, which fontdb does not look in.
fn fonts() -> std::sync::Arc<resvg::usvg::fontdb::Database> {
    static FONTS: std::sync::OnceLock<std::sync::Arc<resvg::usvg::fontdb::Database>> = std::sync::OnceLock::new();
    FONTS
        .get_or_init(|| {
            let mut fonts = resvg::usvg::fontdb::Database::new();
            if cfg!(target_os = "android") {
                fonts.load_fonts_dir("/system/fonts");
            } else {
                fonts.load_system_fonts();
            }
            std::sync::Arc::new(fonts)
        })
        .clone()
}

/// How a drawing is read: with the machine's fonts, and the pictures it names from its own
/// folder -- `folder`, or none if it has none -- or below it, and nowhere else (usvg would read
/// any file it named, from where SpatiLoupe was started if the name was relative).
fn drawing_options(folder: Option<&Path>) -> resvg::usvg::Options<'static> {
    let own = folder.map(|f| if f.as_os_str().is_empty() { Path::new(".") } else { f }).and_then(|f| f.canonicalize().ok());
    let mut options = resvg::usvg::Options { resources_dir: own.clone(), fontdb: fonts(), ..Default::default() };
    let read = resvg::usvg::ImageHrefResolver::default_string_resolver();
    options.image_href_resolver.resolve_string = Box::new(move |href: &str, options: &resvg::usvg::Options| {
        let own = own.as_ref()?;
        let at = own.join(href).canonicalize().ok().filter(|at| at.starts_with(own) && at.is_file());
        match at {
            Some(at) => read(at.to_str()?, options),
            None => {
                log::warn!("a drawing names {href}, which is not a picture in its folder: left out");
                None
            }
        }
    });
    options
}

/// An SVG, drawn so its longer side is `largest`, or its own size if that is smaller and it
/// has one worth keeping. `folder` is where the pictures it names are.
fn drawn(bytes: &[u8], largest: u32, folder: Option<&Path>) -> Result<Picture, String> {
    let options = drawing_options(folder);
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

/// Whether an MPO's second image, `second` pixels, is the other eye of a first of `first`. One the
/// index calls a disparity image is, if it is the first's shape; one it does not -- a phone's
/// Ultra HDR gain map (the first's shape at a quarter its size), its depth map, a preview -- only
/// if it is the first's size as well, near enough.
fn other_eye(what: u32, second: (u32, u32), first: (u32, u32)) -> bool {
    let (sw, sh) = (second.0 as i64, second.1 as i64);
    let (fw, fh) = (first.0 as i64, first.1 as i64);
    let same_shape = (sw * fh - sh * fw).abs() < (fw * fh) / 50;
    let near = |a: i64, b: i64| (a - b).abs() * 50 <= b.max(1);
    same_shape && (what == stereo::DISPARITY || (near(sw, fw) && near(sh, fh)))
}

/// What the camera wrote down, in the words a person would use.
fn about(bytes: &[u8]) -> Vec<(String, String)> {
    about_from(&mut Cursor::new(bytes))
}

/// The same, from a file or anything else that can be read about in: JPEG, TIFF, PNG, WebP, and
/// HEIF and AVIF, the ISO media files phones write.
fn about_from(from: &mut (impl std::io::BufRead + std::io::Seek)) -> Vec<(String, String)> {
    let Ok(exif) = exif::Reader::new().read_from_container(from) else { return Vec::new() };
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

    /// A HEIF of `pictures` bytes of picture with the camera's account (its maker, "Canon") after
    /// them, at its end, as the meta box at its head says.
    fn heif_with_exif(pictures: usize) -> Vec<u8> {
        fn boxed(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
            let mut out = ((body.len() + 8) as u32).to_be_bytes().to_vec();
            out.extend_from_slice(kind);
            out.extend_from_slice(body);
            out
        }
        // A little-endian TIFF of one entry, Make: "Canon".
        let mut tiff = b"II*\0".to_vec();
        tiff.extend_from_slice(&8u32.to_le_bytes());
        tiff.extend_from_slice(&1u16.to_le_bytes());
        tiff.extend_from_slice(&0x010Fu16.to_le_bytes());
        tiff.extend_from_slice(&2u16.to_le_bytes());
        tiff.extend_from_slice(&6u32.to_le_bytes());
        tiff.extend_from_slice(&26u32.to_le_bytes());
        tiff.extend_from_slice(&0u32.to_le_bytes());
        tiff.extend_from_slice(b"Canon\0");
        // Exif's item: where its TIFF starts, then the TIFF.
        let mut exif = 0u32.to_be_bytes().to_vec();
        exif.extend_from_slice(&tiff);
        let ftyp = boxed(b"ftyp", b"heic\0\0\0\0mif1heic");
        let infe = boxed(b"infe", &[&[2, 0, 0, 0][..], &1u16.to_be_bytes(), &0u16.to_be_bytes(), b"Exif", b"\0"].concat());
        let iinf = boxed(b"iinf", &[&[0, 0, 0, 0][..], &1u16.to_be_bytes(), &infe].concat());
        // iloc version 0: offsets and lengths of four bytes, no base offset; its offset is put in
        // once the meta box's size is known.
        let iloc = |offset: u32| boxed(b"iloc", &[&[0, 0, 0, 0][..], &0x4400u16.to_be_bytes(), &1u16.to_be_bytes(), &1u16.to_be_bytes(), &0u16.to_be_bytes(), &1u16.to_be_bytes(), &offset.to_be_bytes(), &(exif.len() as u32).to_be_bytes()].concat());
        let meta = |offset: u32| boxed(b"meta", &[&[0, 0, 0, 0][..], &iinf, &iloc(offset)].concat());
        let head = ftyp.len() + meta(0).len();
        let offset = (head + 8 + pictures) as u32;
        let mut mdat = vec![0x5a; pictures];
        mdat.extend_from_slice(&exif);
        [ftyp, meta(offset), boxed(b"mdat", &mdat)].concat()
    }

    #[test]
    fn a_remote_heif_s_account_is_read_from_its_pieces_not_the_whole_file() {
        let file = heif_with_exif(5 << 20);
        // As read from the file on this machine.
        assert_eq!(about_from(&mut Cursor::new(&file)), [("Camera".to_string(), "Canon".to_string())]);
        // As a remote's, by ranges: its head and its end, a piece each.
        let asked = std::cell::RefCell::new(Vec::new());
        let mut pieces = crate::place::Pieces::new(|start: u64, len: u64| {
            asked.borrow_mut().push((start, len));
            let from = (start as usize).min(file.len());
            let to = (start + len).min(file.len() as u64) as usize;
            Ok((file[from..to].to_vec(), Some(file.len() as u64)))
        });
        assert_eq!(about_from(&mut pieces), [("Camera".to_string(), "Canon".to_string())]);
        assert_eq!(asked.borrow().len(), 2, "{:?}", asked.borrow());
        assert!(pieces.fetched() <= 2 * crate::place::PIECE);
        // One that wants the whole file is refused once it has had the most a head may be.
        let mut whole = crate::place::Pieces::new(|start: u64, len: u64| {
            let from = (start as usize).min(file.len());
            let to = (start + len).min(file.len() as u64) as usize;
            Ok((file[from..to].to_vec(), Some(file.len() as u64)))
        });
        assert!(std::io::Read::read_to_end(&mut whole, &mut Vec::new()).is_err());
        assert!(whole.fetched() <= crate::place::MOST_PIECES);
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

    /// A GIF of two frames, `width` by `height`.
    fn gif(width: u32, height: u32) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut encoder = image::codecs::gif::GifEncoder::new(&mut out);
            for v in [10u8, 200] {
                let frame = image::Frame::from_parts(RgbaImage::from_pixel(width, height, image::Rgba([v, v, v, 255])), 0, 0, image::Delay::from_numer_denom_ms(100, 1));
                encoder.encode_frame(frame).unwrap();
            }
        }
        out
    }

    #[test]
    fn a_moving_picture_moves_and_its_tile_does_not() {
        let bytes = gif(4, 4);
        let dir = std::env::temp_dir().join(format!("spatiloupe-gif-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("two.gif");
        std::fs::write(&path, &bytes).unwrap();
        assert_eq!(read(&path, 100).unwrap().frames.len(), 1);
        assert!(read_still(&path, 100).unwrap().frames.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_gif_that_claims_an_enormous_canvas_is_not_given_it() {
        let mut bytes = gif(1, 1);
        // The logical screen's size, after "GIF89a": 65535 a side, which would be 17 GB a frame.
        bytes[6..10].copy_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF]);
        let started = std::time::Instant::now();
        let read = read_bytes(Path::new("huge.gif"), &bytes, 360);
        assert!(read.is_err(), "refused, not decoded");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn an_ultra_hdr_gain_map_is_not_the_right_eye() {
        // A phone's gain map: the photograph's shape, a quarter its size, of no MP type.
        assert!(!other_eye(0, (1000, 750), (4000, 3000)));
        // A second image of the first's size is the other eye, whatever it is called; one the
        // index calls a disparity image needs only the shape.
        assert!(other_eye(0, (4000, 3000), (4000, 3000)));
        assert!(other_eye(stereo::DISPARITY, (1920, 1440), (4000, 3000)));
        assert!(!other_eye(stereo::DISPARITY, (3000, 3000), (4000, 3000)));
    }

    /// The first pixel of a drawing's that is not clear, if it has one.
    fn drawn_ink(svg: &str, folder: &Path) -> Option<[u8; 4]> {
        let picture = drawn(svg.as_bytes(), 64, Some(folder)).unwrap();
        picture.rgba.chunks_exact(4).find(|p| p[3] > 0).map(|p| [p[0], p[1], p[2], p[3]])
    }

    #[test]
    fn a_drawing_s_text_is_drawn() {
        let dir = std::env::temp_dir();
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="64" height="32"><text x="2" y="26" font-size="28" fill="black">Ag</text></svg>"#;
        assert!(drawn_ink(svg, &dir).is_some(), "the text has ink");
    }

    #[test]
    fn a_drawing_shows_the_pictures_beside_it_and_none_elsewhere() {
        let dir = std::env::temp_dir().join(format!("spatiloupe-svg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("drawing/parts")).unwrap();
        std::fs::write(dir.join("drawing/parts/red.png"), png(4, 4, |_, _| [255, 0, 0, 255])).unwrap();
        std::fs::write(dir.join("outside.png"), png(4, 4, |_, _| [0, 0, 255, 255])).unwrap();
        let svg = |href: &str| format!(r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="8" height="8"><image width="8" height="8" xlink:href="{href}"/></svg>"#);
        let folder = dir.join("drawing");
        assert_eq!(drawn_ink(&svg("parts/red.png"), &folder).map(|p| p[0]), Some(255));
        assert_eq!(drawn_ink(&svg("../outside.png"), &folder), None);
        let outside = dir.join("outside.png");
        assert_eq!(drawn_ink(&svg(outside.to_str().unwrap()), &folder), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_high_dynamic_range_picture_is_given_srgb_s_curve() {
        // Linear 0.214 is sRGB's middle grey, 128; and 1, the brightest here, is white.
        let light = image::Rgb32FImage::from_fn(2, 1, |x, _| if x == 0 { image::Rgb([0.214, 0.214, 0.214]) } else { image::Rgb([1.0, 1.0, 1.0]) });
        let mut bytes = Vec::new();
        DynamicImage::ImageRgb32F(light).write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::OpenExr).unwrap();
        let picture = read_bytes(Path::new("grey.exr"), &bytes, 100).unwrap();
        assert!((picture.rgba[0] as i32 - 128).abs() <= 1, "{}", picture.rgba[0]);
        assert_eq!(picture.rgba[4], 255);
        let bright = image::Rgb32FImage::from_fn(1000, 1, |x, _| image::Rgb([if x < 10 { 4.0 } else { 0.5 }; 3]));
        let shown = shown(DynamicImage::ImageRgb32F(bright));
        // Brighter than 1: the brightest is white, and the rest is brought down to make room.
        assert_eq!(shown.get_pixel(0, 0).0[0], 255);
        assert!(shown.get_pixel(500, 0).0[0] > 150 && shown.get_pixel(500, 0).0[0] < 200, "{:?}", shown.get_pixel(500, 0));
    }

    #[test]
    fn a_misnamed_picture_is_still_read() {
        let bytes = png(8, 8, |_, _| [1, 2, 3, 255]);
        let picture = read_bytes(Path::new("really-a-png.jpg"), &bytes, 100).unwrap();
        assert_eq!(&picture.rgba[..4], &[1, 2, 3, 255]);
    }

    #[test]
    fn a_tga_is_read_by_its_name() {
        // A TGA begins with nothing that says so: it was listed, and never shown.
        let mut bytes = Vec::new();
        DynamicImage::ImageRgba8(RgbaImage::from_pixel(4, 2, image::Rgba([9, 80, 200, 255]))).write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Tga).unwrap();
        let picture = read_bytes(Path::new("scan.tga"), &bytes, 100).unwrap();
        assert_eq!((picture.width, picture.height, &picture.rgba[..4]), (4, 2, &[9, 80, 200, 255][..]));
    }
}

#[cfg(test)]
mod a_picture_here {
    /// A picture on this computer read as SpatiLoupe reads it, and what it makes of its eyes:
    /// `SPATILOUPE_PICTURE=/path/to/picture cargo test -p spatiloupe -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn eyes_of_a_picture() {
        let path = std::path::PathBuf::from(std::env::var("SPATILOUPE_PICTURE").expect("SPATILOUPE_PICTURE"));
        for largest in [super::LARGEST, 384] {
            let picture = super::read(&path, largest).unwrap();
            println!("{largest}: {}x{} (was {:?}): {:?}", picture.width, picture.height, picture.original, picture.eyes);
        }
    }
}
