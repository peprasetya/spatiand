//! A picture marked up, written: drawn on at its own size and its own depth (`depth`), cropped,
//! and written in its own format where SpatiLoupe writes that format as it was read -- over the
//! file, whole or not at all -- or as a copy beside it. (A PDF is marked up and saved in
//! SpatiMesh's viewer, which shows it.)

use std::path::PathBuf;

use image::{DynamicImage, ExtendedColorType, ImageEncoder, ImageFormat};

use crate::depth::{self, Curve};
use crate::ink::{self, Map};
use crate::markup::Marks;

/// How a JPEG is written: high, as a photograph written again should be.
const JPEG_QUALITY: u8 = 92;

/// The formats a picture is written back in as it is. JPEG is written again at high quality,
/// PNG at its own depth (sixteen bits where it had them), WebP without loss (the image crate's
/// only WebP); TIFF, OpenEXR and Radiance HDR as they were read, where SpatiLoupe can
/// (`depth::tiff_as_read` and the rest say, of the file). Any other -- HEIC, AVIF, JPEG XL,
/// JPEG 2000, GIF, a drawing -- is saved as a copy.
pub fn in_place(name: &str) -> Option<ImageFormat> {
    let extension = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase())?;
    match extension.as_str() {
        "jpg" | "jpeg" | "jpe" | "jfif" => Some(ImageFormat::Jpeg),
        "png" => Some(ImageFormat::Png),
        "webp" => Some(ImageFormat::WebP),
        "bmp" => Some(ImageFormat::Bmp),
        "tif" | "tiff" => Some(ImageFormat::Tiff),
        "exr" => Some(ImageFormat::OpenExr),
        "hdr" => Some(ImageFormat::Hdr),
        _ => None,
    }
}

/// How deep a picture's samples are, for what a copy of it is written as.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Depth {
    Eight,
    /// Ten, twelve or sixteen bits.
    Sixteen,
    /// Floats: light itself.
    Float,
}

impl Depth {
    pub fn of(image: &DynamicImage) -> Depth {
        match image {
            DynamicImage::ImageRgb32F(_) | DynamicImage::ImageRgba32F(_) => Depth::Float,
            DynamicImage::ImageLuma16(_) | DynamicImage::ImageLumaA16(_) | DynamicImage::ImageRgb16(_) | DynamicImage::ImageRgba16(_) => Depth::Sixteen,
            _ => Depth::Eight,
        }
    }
}

/// Why a picture SpatiLoupe writes the format of is not written over as it is, said of its
/// bytes: a TIFF, an OpenEXR or a Radiance HDR it would not write back as it was read. `None`
/// for any other.
pub fn not_over(name: &str, bytes: &[u8]) -> Option<String> {
    match in_place(name)? {
        ImageFormat::Tiff => depth::tiff_as_read(bytes).err(),
        ImageFormat::OpenExr => depth::exr_as_read(bytes).err(),
        ImageFormat::Hdr => depth::hdr_as_read(bytes).err(),
        _ => None,
    }
}

/// What a copy is called: "<stem> (edited).<extension>", or "(edited 2)" and on if that is
/// taken. `extension` is the copy's, if it is not the original's.
pub fn copy_name(name: &str, extension: Option<&str>, taken: impl Fn(&str) -> bool) -> String {
    let (stem, own) = match name.rsplit_once('.') {
        Some((stem, own)) if !stem.is_empty() => (stem, Some(own)),
        _ => (name, None),
    };
    let extension = extension.or(own);
    let mut n = 1;
    loop {
        let mark = if n == 1 { " (edited)".to_string() } else { format!(" (edited {n})") };
        let candidate = match extension {
            Some(e) => format!("{stem}{mark}.{e}"),
            None => format!("{stem}{mark}"),
        };
        if !taken(&candidate) || n > 9999 {
            return candidate;
        }
        n += 1;
    }
}

/// What a copy of a picture is written as: its own format where it can be; else, deeper than
/// eight bits, what keeps its depth without loss -- a sixteen-bit PNG, or for floats an OpenEXR
/// -- and otherwise JPEG, or PNG for a picture with see-through parts, which JPEG has not.
/// `own` is whether its own format is written as it was read (`not_over`).
pub fn copy_format(name: &str, clear: bool, depth: Depth, own: bool) -> (ImageFormat, Option<&'static str>) {
    match (in_place(name), depth) {
        (Some(format), _) if own => (format, None),
        // Floats in a TIFF it does not write as it was: still a TIFF, of floats.
        (Some(ImageFormat::Tiff), Depth::Float) => (ImageFormat::Tiff, None),
        (_, Depth::Float) => (ImageFormat::OpenExr, Some("exr")),
        (_, Depth::Sixteen) => (ImageFormat::Png, Some("png")),
        _ if clear => (ImageFormat::Png, Some("png")),
        _ => (ImageFormat::Jpeg, Some("jpg")),
    }
}

/// What the foot says of a picture SpatiLoupe only copies: why, and what the copy is.
pub fn copy_only(name: &str, why: Option<&str>, depth: Depth) -> String {
    let kind = name.rsplit_once('.').map_or(String::new(), |(_, e)| format!("{} ", e.to_uppercase()));
    let copy = match (copy_format(name, false, depth, false).0, depth) {
        (ImageFormat::Png, Depth::Sixteen) => "a 16-bit PNG beside it, which keeps its depth",
        (ImageFormat::Tiff, _) => "a TIFF of its floats beside it",
        (ImageFormat::OpenExr, _) => "an OpenEXR beside it, which keeps its light as it is",
        _ => "a JPEG beside it, or a PNG if it has see-through parts",
    };
    match why {
        Some(why) => format!("SpatiLoupe does not write this {kind}picture over itself: {why}. Save a copy writes {copy}."),
        None => format!("SpatiLoupe does not write {kind}pictures: Save a copy writes {copy}."),
    }
}

/// The camera's account of a picture written the right way up and `size` large: its orientation
/// reset to upright (the pixels are written turned already), the thumbnail it carried dropped
/// (it would show the picture as it was), and the size it gives brought up to date. What is
/// not as a TIFF should be is left as it was.
pub fn exif_upright(mut exif: Vec<u8>, size: (u32, u32)) -> Vec<u8> {
    let big = match exif.get(0..2) {
        Some(b"MM") => true,
        Some(b"II") => false,
        _ => return exif,
    };
    fn get16(b: &[u8], at: usize, big: bool) -> Option<u16> {
        let s: [u8; 2] = b.get(at..at + 2)?.try_into().ok()?;
        Some(if big { u16::from_be_bytes(s) } else { u16::from_le_bytes(s) })
    }
    fn get32(b: &[u8], at: usize, big: bool) -> Option<u32> {
        let s: [u8; 4] = b.get(at..at + 4)?.try_into().ok()?;
        Some(if big { u32::from_be_bytes(s) } else { u32::from_le_bytes(s) })
    }
    fn put16(b: &mut [u8], at: usize, big: bool, v: u16) {
        if let Some(s) = b.get_mut(at..at + 2) {
            s.copy_from_slice(&if big { v.to_be_bytes() } else { v.to_le_bytes() });
        }
    }
    fn put32(b: &mut [u8], at: usize, big: bool, v: u32) {
        if let Some(s) = b.get_mut(at..at + 4) {
            s.copy_from_slice(&if big { v.to_be_bytes() } else { v.to_le_bytes() });
        }
    }
    // The entries of the IFD at `at`: where each is, its tag and its type.
    let entries = |b: &[u8], at: usize| -> Option<Vec<(usize, u16, u16)>> {
        let n = get16(b, at, big)? as usize;
        (0..n).map(|i| at + 2 + 12 * i).map(|e| Some((e, get16(b, e, big)?, get16(b, e + 2, big)?))).collect()
    };
    let Some(ifd0) = get32(&exif, 4, big).map(|o| o as usize) else { return exif };
    let Some(first) = entries(&exif, ifd0) else { return exif };
    let mut exif_ifd = None;
    for &(entry, tag, kind) in &first {
        match (tag, kind) {
            (0x0112, 3) => put16(&mut exif, entry + 8, big, 1),
            (0x8769, 4) => exif_ifd = get32(&exif, entry + 8, big).map(|o| o as usize),
            _ => {}
        }
    }
    // No next IFD: no thumbnail.
    put32(&mut exif, ifd0 + 2 + 12 * first.len(), big, 0);
    if let Some(inner) = exif_ifd.and_then(|at| entries(&exif, at)) {
        for (entry, tag, kind) in inner {
            let value = match tag {
                0xA002 => size.0,
                0xA003 => size.1,
                _ => continue,
            };
            match kind {
                3 if value <= u16::MAX as u32 => put16(&mut exif, entry + 8, big, value as u16),
                4 => put32(&mut exif, entry + 8, big, value),
                _ => {}
            }
        }
    }
    exif
}

/// `image` in `format` at its own depth where the format keeps it, with the file's colour profile
/// and camera's account where the format carries them -- or, for a PNG of a picture that named
/// its colours rather than profiled them (a HEIF's or AVIF's nclx), their code points (cICP).
fn encode(image: &DynamicImage, format: ImageFormat, whole: &crate::picture::Whole) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let (w, h) = (image.width(), image.height());
    let opaque = opaque(image);
    let exif = whole.exif.clone().map(|exif| exif_upright(exif, (w, h)));
    let icc = whole.icc.clone();
    fn finish(mut encoder: impl ImageEncoder, data: &[u8], (w, h): (u32, u32), kind: ExtendedColorType, exif: Option<Vec<u8>>, icc: Option<Vec<u8>>) -> Result<(), String> {
        // Formats that do not carry them say so; the picture is written all the same.
        if let Some(icc) = icc {
            let _ = encoder.set_icc_profile(icc);
        }
        if let Some(exif) = exif {
            let _ = encoder.set_exif_metadata(exif);
        }
        encoder.write_image(data, w, h, kind).map_err(|e| e.to_string())
    }
    // Eight bits a sample, as JPEG, WebP and BMP have them; three channels without see-through
    // parts: a third smaller, and what a photograph is.
    let eight = |alpha: bool| if alpha && !opaque { (image.to_rgba8().into_raw(), ExtendedColorType::Rgba8) } else { (image.to_rgb8().into_raw(), ExtendedColorType::Rgb8) };
    match format {
        ImageFormat::Jpeg => {
            let (data, kind) = eight(false);
            finish(image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY), &data, (w, h), kind, exif, icc)?;
        }
        ImageFormat::WebP => {
            let (data, kind) = eight(true);
            finish(image::codecs::webp::WebPEncoder::new_lossless(&mut out), &data, (w, h), kind, exif, icc)?;
        }
        ImageFormat::Bmp => {
            let (data, kind) = eight(true);
            finish(image::codecs::bmp::BmpEncoder::new(&mut out), &data, (w, h), kind, exif, icc)?;
        }
        ImageFormat::Png => {
            // At its own depth: eight or sixteen bits, grey or coloured, see-through only if it is.
            let image = match image {
                DynamicImage::ImageRgba8(_) | DynamicImage::ImageLumaA8(_) if opaque => DynamicImage::ImageRgb8(image.to_rgb8()),
                DynamicImage::ImageRgba16(_) | DynamicImage::ImageLumaA16(_) if opaque => DynamicImage::ImageRgb16(image.to_rgb16()),
                DynamicImage::ImageRgb32F(_) | DynamicImage::ImageRgba32F(_) => DynamicImage::ImageRgba16(image.to_rgba16()),
                other => other.clone(),
            };
            let kind = image.color().into();
            finish(image::codecs::png::PngEncoder::new(&mut out), image.as_bytes(), (w, h), kind, exif, icc.clone())?;
            if let (None, Some(cicp)) = (&icc, whole.cicp) {
                out = depth::with_cicp(out, cicp);
            }
        }
        ImageFormat::Tiff => match depth::tiff_as_read(&whole.bytes) {
            // As it was read: its compression, resolution, profile and words.
            Ok(asread) => out = depth::tiff_bytes(image, &asread)?,
            // A copy of a TIFF it does not write back as it was: the image crate's own, at its
            // depth, uncompressed, with its profile.
            Err(_) => {
                let mut cursor = std::io::Cursor::new(Vec::new());
                let kind = image.color().into();
                finish(image::codecs::tiff::TiffEncoder::new(&mut cursor), image.as_bytes(), (w, h), kind, None, icc)?;
                out = cursor.into_inner();
            }
        },
        ImageFormat::OpenExr => {
            // Light, red, green, blue (and alpha), as floats: the image crate's own.
            let mut cursor = std::io::Cursor::new(Vec::new());
            let (data, kind) = if opaque { (image.to_rgb32f().into_raw(), ExtendedColorType::Rgb32F) } else { (image.to_rgba32f().into_raw(), ExtendedColorType::Rgba32F) };
            let bytes: Vec<u8> = data.iter().flat_map(|v| v.to_ne_bytes()).collect();
            finish(image::codecs::openexr::OpenExrEncoder::new(&mut cursor), &bytes, (w, h), kind, None, None)?;
            out = cursor.into_inner();
        }
        ImageFormat::Hdr => out = depth::hdr_bytes(image, &whole.bytes)?,
        other => return Err(format!("SpatiLoupe does not write {other:?}")),
    }
    Ok(out)
}

/// A picture to be written with its marks.
pub struct Saving {
    pub source: PathBuf,
    pub marks: Marks,
    /// The picture's size as it was shown, in its own pixels and turned as it was turned: the
    /// units the marks are in.
    pub frame: (f32, f32),
    /// A copy beside it, rather than over it.
    pub copy: bool,
}

/// A picture as its marks make it: turned, the marks' layers at its turned size, and what is kept
/// of it (x, y, width, height in the turned picture's pixels).
pub struct Rendered {
    pub image: DynamicImage,
    pub whole: crate::picture::Whole,
    pub turns: u8,
    pub layers: ink::Layers,
    pub turned_size: (u32, u32),
    pub crop: Option<(u32, u32, u32, u32)>,
}

/// The picture of `saving.source` at its own size and depth, turned, drawn on and cropped as its
/// marks say -- what is written -- and, from the file, its camera's account, its colours and the
/// format it is in.
pub fn render(saving: &Saving) -> Result<Rendered, String> {
    let mut whole = crate::picture::read_whole(&saving.source)?;
    // The curve its samples are on, which the marks' colours are put on.
    let curve = Curve::of(&whole);
    let image = std::mem::replace(&mut whole.image, DynamicImage::new_rgba8(0, 0));
    let turns = saving.marks.turns % 4;
    let image = depth::turned(image, turns);
    let (w, h) = (image.width(), image.height());
    // The marks' units to this picture's pixels: one to one but for a drawing, drawn at its
    // shown size rather than its own.
    let scale = w as f32 / saving.frame.0.max(1e-3);
    let notes: Vec<_> = saving.marks.notes.iter().filter(|n| n.page == 0).collect();
    let layers = ink::layer(w, h, &notes, Map { scale, dx: 0.0, dy: 0.0 })?;
    let mut image = depth::lay(image, &layers, curve);
    let crop = saving.marks.crop(0).map(|crop| {
        let x0 = (crop.x0 * scale).round().clamp(0.0, w as f32 - 1.0) as u32;
        let y0 = (crop.y0 * scale).round().clamp(0.0, h as f32 - 1.0) as u32;
        let x1 = ((crop.x1 * scale).round() as u32).clamp(x0 + 1, w);
        let y1 = ((crop.y1 * scale).round() as u32).clamp(y0 + 1, h);
        (x0, y0, x1 - x0, y1 - y0)
    });
    if let Some((x, y, cw, ch)) = crop {
        image = image.crop_imm(x, y, cw, ch);
    }
    Ok(Rendered { image, whole, turns, layers, turned_size: (w, h), crop })
}

/// Whether a picture has no see-through parts at all.
fn opaque(image: &DynamicImage) -> bool {
    match image {
        DynamicImage::ImageRgba8(b) => b.pixels().all(|p| p.0[3] == 255),
        DynamicImage::ImageLumaA8(b) => b.pixels().all(|p| p.0[1] == 255),
        DynamicImage::ImageRgba16(b) => b.pixels().all(|p| p.0[3] == u16::MAX),
        DynamicImage::ImageLumaA16(b) => b.pixels().all(|p| p.0[1] == u16::MAX),
        DynamicImage::ImageRgba32F(b) => b.pixels().all(|p| p.0[3] >= 1.0),
        _ => true,
    }
}

/// Write the picture marked up: over itself in its own format, or as a copy beside it. Where it
/// was written.
pub fn save(saving: &Saving) -> Result<PathBuf, String> {
    let name = crate::place::name(&saving.source);
    if !saving.copy && in_place(&name).is_none() {
        return Err(format!("SpatiLoupe does not write pictures like {name}: Save a copy writes one beside it"));
    }
    let rendered = render(saving)?;
    let (image, whole) = (&rendered.image, &rendered.whole);
    // Its own format, written as it was read: what is written over it, and its copy's format.
    let own = in_place(&name).is_some_and(|format| whole.format == Some(format)) && not_over(&name, &whole.bytes).is_none();
    let (format, extension) = if saving.copy {
        copy_format(&name, !opaque(image), Depth::of(image), own)
    } else {
        let format = in_place(&name).ok_or("no format")?;
        if whole.format != Some(format) {
            return Err(format!("{name} is not the kind of picture its name says: Save a copy writes it as one"));
        }
        if let Some(why) = not_over(&name, &whole.bytes) {
            return Err(format!("SpatiLoupe does not write {name} over itself, as {why}: Save a copy writes it beside it"));
        }
        (format, None)
    };
    // An OpenEXR written as it was: every channel of its layer, not only the picture of it.
    let mut bytes = if format == ImageFormat::OpenExr && own {
        depth::exr_bytes(&whole.bytes, rendered.turns, &rendered.layers, rendered.crop)?
    } else {
        encode(image, format, whole)?
    };
    // A JPEG's HDR gain map, made as the picture was and written after it; one that cannot be
    // carried is left (the question before the save said so), and the picture is SDR.
    if format == ImageFormat::Jpeg && whole.format == Some(ImageFormat::Jpeg) {
        match crate::gainmap::find(&whole.bytes) {
            crate::gainmap::Found::Carried(gain) => {
                // Under the marks no brighter than paper white; under a highlighter as bright as
                // ever, which it tints in HDR as on paper.
                let map = crate::gainmap::remade(&gain, whole.orientation, rendered.turns, &rendered.layers.over, rendered.turned_size, rendered.crop)?;
                bytes = crate::gainmap::written(&bytes, &gain, &map)?;
            }
            // Its XMP too (its rating, its words), upright: what the image crate does not keep.
            crate::gainmap::Found::None => bytes = crate::gainmap::with_xmp(&bytes, &whole.bytes),
            // An HDR layer's own description goes with it.
            crate::gainmap::Found::Lost(_) => {}
        }
    }
    if saving.copy {
        let folder = crate::place::parent(&saving.source).ok_or("it is in no folder")?;
        let copy = copy_name(&name, extension, |candidate| crate::place::exists(&crate::place::join(&folder, candidate)));
        let path = crate::place::join(&folder, &copy);
        crate::place::write_new(&path, &bytes)?;
        return Ok(path);
    }
    crate::place::replace(&saving.source, &bytes)?;
    Ok(saving.source.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markup::{Area, Brush, Face, Note, Pick, Shape, HIGHLIGHTER_COLOURS, PEN_COLOURS};
    use image::RgbaImage;

    #[test]
    fn a_picture_is_saved_with_a_pen_s_line_and_a_highlighter_as_they_were_seen() {
        let dir = folder("strokes");
        let path = dir.join("page.png");
        // White above, black below.
        RgbaImage::from_fn(200, 100, |_, y| if y < 50 { image::Rgba([255, 255, 255, 255]) } else { image::Rgba([0, 0, 0, 255]) }).save(&path).unwrap();
        let pen = Note { pick: Pick::Chromium(6.0), ..Note::new(0, Shape::Stroke { brush: Brush::Pen, points: vec![(10.0, 20.0), (100.0, 20.0), (190.0, 20.0)] }, PEN_COLOURS[13].colour, 6.0) };
        // Down across the pen's line, after it, and on into the black.
        let marker = Note { pick: Pick::Chromium(8.0), ..Note::new(0, Shape::Stroke { brush: Brush::Highlighter, points: vec![(100.0, 5.0), (100.0, 95.0)] }, HIGHLIGHTER_COLOURS[1].colour, 16.0) };
        let marks = Marks { notes: vec![pen, marker], ..Marks::default() };
        save(&Saving { source: path.clone(), marks, frame: (200.0, 100.0), copy: false }).unwrap();
        let read = image::open(&path).unwrap().to_rgba8();
        assert_eq!(read.get_pixel(40, 20).0, [0x42, 0x85, 0xf4, 255], "the pen's Blue 2 along its line");
        assert_eq!(read.get_pixel(100, 35).0, [0xfe, 0xef, 0xc1, 255], "the highlighter on white, as Chromium lays it on paper");
        assert_eq!(read.get_pixel(100, 70).0, [0, 0, 0, 255], "on black, black as under a highlighter's ink");
        // Over the pen's line, which it was drawn after: the blue under it tinted by it.
        let under = read.get_pixel(100, 20).0;
        let tinted = [0x42 as f32 * 254.0 / 255.0, 0x85 as f32 * 239.0 / 255.0, 0xf4 as f32 * 193.0 / 255.0];
        assert!((0..3).all(|k| (under[k] as f32 - tinted[k]).abs() <= 1.5), "{under:?} {tinted:?}");
        assert_eq!(read.get_pixel(150, 40).0, [255, 255, 255, 255], "untouched elsewhere");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A little-endian or big-endian TIFF: IFD0 with the orientation (6, a quarter turn) and the
    /// Exif IFD's pointer, a next IFD (a thumbnail's); the Exif IFD with the pixel size, as a
    /// SHORT and a LONG.
    fn tiff(big: bool) -> Vec<u8> {
        let w16 = |v: u16| if big { v.to_be_bytes().to_vec() } else { v.to_le_bytes().to_vec() };
        let w32 = |v: u32| if big { v.to_be_bytes().to_vec() } else { v.to_le_bytes().to_vec() };
        let mut t = if big { b"MM\0*".to_vec() } else { b"II*\0".to_vec() };
        t.extend(w32(8));
        // IFD0 at 8: two entries, then the next IFD at 68.
        t.extend(w16(2));
        t.extend(w16(0x0112));
        t.extend(w16(3));
        t.extend(w32(1));
        t.extend(w16(6));
        t.extend(w16(0));
        t.extend(w16(0x8769));
        t.extend(w16(4));
        t.extend(w32(1));
        t.extend(w32(38));
        t.extend(w32(68));
        // The Exif IFD at 38: two entries, no next.
        t.extend(w16(2));
        t.extend(w16(0xA002));
        t.extend(w16(3));
        t.extend(w32(1));
        t.extend(w16(4000));
        t.extend(w16(0));
        t.extend(w16(0xA003));
        t.extend(w16(4));
        t.extend(w32(1));
        t.extend(w32(3000));
        t.extend(w32(0));
        assert_eq!(t.len(), 68);
        // A thumbnail's IFD at 68, no entries.
        t.extend(w16(0));
        t.extend(w32(0));
        t
    }

    #[test]
    fn the_camera_s_account_is_made_upright_its_thumbnail_dropped_and_its_size_true() {
        for big in [false, true] {
            let fixed = exif_upright(tiff(big), (1200, 900));
            let r16 = |at: usize| if big { u16::from_be_bytes([fixed[at], fixed[at + 1]]) } else { u16::from_le_bytes([fixed[at], fixed[at + 1]]) };
            let r32 = |at: usize| if big { u32::from_be_bytes(fixed[at..at + 4].try_into().unwrap()) } else { u32::from_le_bytes(fixed[at..at + 4].try_into().unwrap()) };
            assert_eq!(r16(18), 1, "upright");
            assert_eq!(r32(34), 0, "no thumbnail");
            assert_eq!((r16(48), r32(60)), (1200, 900), "the size written");
            // As kamadak-exif reads it.
            let read = exif::Reader::new().read_raw(fixed.clone()).unwrap();
            assert_eq!(read.get_field(exif::Tag::Orientation, exif::In::PRIMARY).and_then(|f| f.value.get_uint(0)), Some(1));
        }
        // Not a TIFF: left as it was.
        assert_eq!(exif_upright(b"Exif\0\0junk".to_vec(), (1, 1)), b"Exif\0\0junk".to_vec());
        assert_eq!(exif_upright(b"II*\0\xff\xff\xff\xff".to_vec(), (1, 1)), b"II*\0\xff\xff\xff\xff".to_vec());
    }

    #[test]
    fn a_copy_is_named_beside_its_original() {
        let none = |_: &str| false;
        assert_eq!(copy_name("IMG_1024.JPG", None, none), "IMG_1024 (edited).JPG");
        assert_eq!(copy_name("IMG_1024.HEIC", Some("jpg"), none), "IMG_1024 (edited).jpg");
        let taken = ["a (edited).png", "a (edited 2).png"];
        assert_eq!(copy_name("a.png", None, |n| taken.contains(&n)), "a (edited 3).png");
        assert_eq!(copy_name("notes", None, none), "notes (edited)");
        assert_eq!(copy_name(".hidden", Some("png"), none), ".hidden (edited).png");
        assert_eq!(copy_name("trip.2024.png", None, none), "trip.2024 (edited).png");
    }

    #[test]
    fn which_pictures_are_written_back_as_they_are() {
        assert_eq!(in_place("a.JPG"), Some(ImageFormat::Jpeg));
        assert_eq!(in_place("a.jpeg"), Some(ImageFormat::Jpeg));
        assert_eq!(in_place("a.png"), Some(ImageFormat::Png));
        assert_eq!(in_place("a.webp"), Some(ImageFormat::WebP));
        assert_eq!(in_place("a.bmp"), Some(ImageFormat::Bmp));
        // As they were read, where they can be (`not_over` says, of the file).
        assert_eq!(in_place("a.tif"), Some(ImageFormat::Tiff));
        assert_eq!(in_place("a.exr"), Some(ImageFormat::OpenExr));
        assert_eq!(in_place("a.hdr"), Some(ImageFormat::Hdr));
        for other in ["a.heic", "a.avif", "a.jxl", "a.jp2", "a.gif", "a.svg", "a"] {
            assert_eq!(in_place(other), None, "{other}");
        }
        let eight = Depth::Eight;
        assert_eq!(copy_format("a.heic", false, eight, false), (ImageFormat::Jpeg, Some("jpg")));
        assert_eq!(copy_format("a.heic", true, eight, false), (ImageFormat::Png, Some("png")));
        assert_eq!(copy_format("a.webp", true, eight, true), (ImageFormat::WebP, None));
        // Deeper: what keeps the depth, without loss.
        assert_eq!(copy_format("a.heic", false, Depth::Sixteen, false), (ImageFormat::Png, Some("png")));
        assert_eq!(copy_format("a.png", false, Depth::Sixteen, true), (ImageFormat::Png, None));
        assert_eq!(copy_format("a.tif", false, Depth::Sixteen, false), (ImageFormat::Png, Some("png")));
        assert_eq!(copy_format("a.tif", false, Depth::Sixteen, true), (ImageFormat::Tiff, None));
        assert_eq!(copy_format("a.exr", false, Depth::Float, false), (ImageFormat::OpenExr, Some("exr")));
        assert_eq!(copy_format("a.tif", false, Depth::Float, false), (ImageFormat::Tiff, None));
        // And what the foot says of one only copied.
        assert_eq!(copy_only("IMG_1.HEIC", None, Depth::Sixteen), "SpatiLoupe does not write HEIC pictures: Save a copy writes a 16-bit PNG beside it, which keeps its depth.");
        assert_eq!(copy_only("a.heic", None, eight), "SpatiLoupe does not write HEIC pictures: Save a copy writes a JPEG beside it, or a PNG if it has see-through parts.");
        assert_eq!(copy_only("scan.tif", Some("it holds more than one picture"), Depth::Sixteen), "SpatiLoupe does not write this TIF picture over itself: it holds more than one picture. Save a copy writes a 16-bit PNG beside it, which keeps its depth.");
    }

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("spatiloupe-save-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A box in the pen's Red 2.
    fn red_box(area: Area, size: f32) -> Note {
        Note::new(0, Shape::Box(area), PEN_COLOURS[10].colour, size)
    }

    fn reddish(p: &image::Rgba<u8>) -> bool {
        p.0[0] > 180 && p.0[1] < 110 && p.0[2] < 110
    }

    /// A JPEG taken on its side: stored 40 wide and 20 tall, its camera saying to turn it a
    /// quarter clockwise, so shown 20 wide and 40 tall. Grey, with a white top row as stored.
    fn sideways_jpeg(path: &std::path::Path) {
        let image = RgbaImage::from_fn(40, 20, |_, y| if y == 0 { image::Rgba([255, 255, 255, 255]) } else { image::Rgba([90, 90, 90, 255]) });
        let mut out = Vec::new();
        let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 95);
        encoder.set_exif_metadata(tiff(false)).unwrap();
        encoder.write_image(&DynamicImage::ImageRgba8(image).to_rgb8(), 40, 20, ExtendedColorType::Rgb8).unwrap();
        std::fs::write(path, out).unwrap();
    }

    #[test]
    fn a_jpeg_is_saved_upright_with_its_marks_and_its_account_and_the_copy_beside_it() {
        let dir = folder("jpeg");
        let path = dir.join("side.jpg");
        sideways_jpeg(&path);
        let original = std::fs::read(&path).unwrap();
        // As shown: 20 wide, 40 tall. A box round the top half's middle, 4 wide (JPEG halves the
        // colour's sharpness across).
        let marks = Marks { notes: vec![red_box(Area::new(4.0, 4.0, 16.0, 16.0), 4.0)], ..Marks::default() };
        let copy = save(&Saving { source: path.clone(), marks: marks.clone(), frame: (20.0, 40.0), copy: true }).unwrap();
        assert_eq!(copy, dir.join("side (edited).jpg"));
        assert_eq!(std::fs::read(&path).unwrap(), original, "the original untouched");
        let read = image::open(&copy).unwrap().to_rgba8();
        assert_eq!(read.dimensions(), (20, 40), "written upright, as shown");
        assert!(reddish(read.get_pixel(4, 10)), "the box's left edge: {:?}", read.get_pixel(4, 10));
        assert!(!reddish(read.get_pixel(10, 10)), "inside, not");
        // The camera's account kept, upright.
        let exif = exif::Reader::new().read_from_container(&mut std::io::BufReader::new(std::fs::File::open(&copy).unwrap())).unwrap();
        assert_eq!(exif.get_field(exif::Tag::Orientation, exif::In::PRIMARY).and_then(|f| f.value.get_uint(0)), Some(1));
        assert_eq!(exif.get_field(exif::Tag::PixelXDimension, exif::In::PRIMARY).and_then(|f| f.value.get_uint(0)), Some(20));
        // A second copy is named after the first.
        let second = save(&Saving { source: path.clone(), marks: marks.clone(), frame: (20.0, 40.0), copy: true }).unwrap();
        assert_eq!(second, dir.join("side (edited 2).jpg"));
        // Saved over itself, cropped to the box: 12 by 12 and red at its edge; no file left over.
        let mut cropped = marks.clone();
        cropped.crops = vec![(0, Area::new(4.0, 4.0, 16.0, 16.0))];
        assert_eq!(save(&Saving { source: path.clone(), marks: cropped, frame: (20.0, 40.0), copy: false }).unwrap(), path);
        let read = image::open(&path).unwrap().to_rgba8();
        assert_eq!(read.dimensions(), (12, 12));
        assert!(reddish(read.get_pixel(0, 6)), "{:?}", read.get_pixel(0, 6));
        let names: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(names.len(), 3, "{names:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_jpeg_s_xmp_is_kept_upright() {
        let dir = folder("xmp");
        let path = dir.join("rated.jpg");
        sideways_jpeg(&path);
        // Its XMP: a rating, words, and the orientation as a camera's software writes it.
        let xmp = b"http://ns.adobe.com/xap/1.0/\0<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"><rdf:Description xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\" xmlns:tiff=\"http://ns.adobe.com/tiff/1.0/\" xmp:Rating=\"4\" tiff:Orientation=\"6\"/></rdf:RDF></x:xmpmeta>";
        let jpeg = std::fs::read(&path).unwrap();
        let mut segment = vec![0xFF, 0xE1];
        segment.extend_from_slice(&((xmp.len() + 2) as u16).to_be_bytes());
        segment.extend_from_slice(xmp);
        std::fs::write(&path, [&jpeg[..2], &segment, &jpeg[2..]].concat()).unwrap();
        let marks = Marks { notes: vec![red_box(Area::new(4.0, 4.0, 16.0, 16.0), 4.0)], ..Marks::default() };
        save(&Saving { source: path.clone(), marks, frame: (20.0, 40.0), copy: false }).unwrap();
        let written = String::from_utf8_lossy(&std::fs::read(&path).unwrap()).into_owned();
        assert!(written.contains("xmp:Rating=\"4\"") && written.contains("tiff:Orientation=\"1\""), "kept, upright");
        assert!(!written.contains("tiff:Orientation=\"6\""));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_picture_turned_is_saved_turned_and_its_permissions_are_kept() {
        let dir = folder("turned");
        let path = dir.join("wide.png");
        // 30 wide, 10 tall, see-through on the right third.
        let image = RgbaImage::from_fn(30, 10, |x, _| if x >= 20 { image::Rgba([0, 0, 0, 0]) } else { image::Rgba([200, 200, 200, 255]) });
        image.save(&path).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
        }
        // Turned a quarter clockwise: shown 10 wide and 30 tall, the see-through part now at the
        // foot. An arrow down its middle, ending in the clear part.
        let marks = Marks {
            notes: vec![Note::new(0, Shape::Arrow { from: (5.0, 2.0), to: (5.0, 28.0) }, PEN_COLOURS[10].colour, 2.0)],
            crops: Vec::new(),
            turns: 1,
        };
        save(&Saving { source: path.clone(), marks, frame: (10.0, 30.0), copy: false }).unwrap();
        let read = image::open(&path).unwrap().to_rgba8();
        assert_eq!(read.dimensions(), (10, 30));
        assert!(reddish(read.get_pixel(5, 10)));
        assert_eq!(read.get_pixel(1, 28).0[3], 0, "still see-through beside the arrow");
        assert_eq!(read.get_pixel(5, 22).0[3], 255, "the arrow's head is ink on the clear part");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o640);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_format_not_written_is_copied_as_jpeg_or_png() {
        let dir = folder("copyonly");
        let path = dir.join("scan.tga");
        RgbaImage::from_pixel(16, 8, image::Rgba([10, 200, 10, 255])).save(&path).unwrap();
        let marks = Marks { notes: vec![red_box(Area::new(2.0, 2.0, 14.0, 6.0), 2.0)], ..Marks::default() };
        let refused = save(&Saving { source: path.clone(), marks: marks.clone(), frame: (16.0, 8.0), copy: false });
        assert!(refused.unwrap_err().contains("Save a copy"));
        let copy = save(&Saving { source: path.clone(), marks: marks.clone(), frame: (16.0, 8.0), copy: true }).unwrap();
        assert_eq!(copy, dir.join("scan (edited).jpg"));
        assert_eq!(image::ImageReader::open(&copy).unwrap().with_guessed_format().unwrap().format(), Some(ImageFormat::Jpeg));
        // With see-through parts: PNG.
        let clear = dir.join("icon.tga");
        RgbaImage::from_pixel(16, 8, image::Rgba([10, 200, 10, 0])).save(&clear).unwrap();
        let copy = save(&Saving { source: clear, marks, frame: (16.0, 8.0), copy: true }).unwrap();
        assert_eq!(copy, dir.join("icon (edited).png"));
        let read = image::open(&copy).unwrap().to_rgba8();
        assert_eq!(read.get_pixel(8, 0).0[3], 0);
        assert_eq!(read.get_pixel(2, 4).0[3], 255, "the box's edge");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn words_come_out_in_the_saved_picture() {
        let dir = folder("words");
        let path = dir.join("plain.png");
        RgbaImage::from_pixel(400, 120, image::Rgba([255, 255, 255, 255])).save(&path).unwrap();
        let words = Note::new(0, Shape::Text { at: (10.0, 10.0), words: "Hello Привет 你好".into(), extent: (0.0, 0.0), face: Face::default() }, PEN_COLOURS[13].colour, 40.0);
        let marks = Marks { notes: vec![words], ..Marks::default() };
        save(&Saving { source: path.clone(), marks, frame: (400.0, 120.0), copy: false }).unwrap();
        let read = image::open(&path).unwrap().to_rgba8();
        let blue = |x0: u32, x1: u32| (10..70).flat_map(|y| (x0..x1).map(move |x| (x, y))).filter(|&(x, y)| read.get_pixel(x, y).0[2] > 150 && read.get_pixel(x, y).0[0] < 120).count();
        // Latin, Cyrillic and Chinese each put ink where they are set.
        let (latin, _) = crate::ink::measure(&mut crate::ink::fonts(), "Hello ", 40.0, &Face::default());
        let (both, _) = crate::ink::measure(&mut crate::ink::fonts(), "Hello Привет ", 40.0, &Face::default());
        let (latin, both) = (10 + latin as u32, 10 + both as u32);
        assert!(blue(10, latin) > 100, "Latin");
        assert!(blue(latin, both) > 100, "Cyrillic");
        assert!(blue(both, 400) > 100, "Chinese");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Pictures FFmpeg reads, made on deepmagpie with what it has (testdata/pictures/make.py: x265,
/// libaom, libjxl and OpenJPEG through ffmpeg, the HEIF container written by hand around x265's
/// HEVC, there being no HEIF encoder there), of one pattern stored 480 by 360: four quarters --
/// red, green, blue, light grey -- over a grey ramp 40 tall along the foot.
#[cfg(test)]
pub(crate) mod pictures {
    use std::path::{Path, PathBuf};

    pub const QUARTERS: [[u8; 3]; 4] = [[200, 40, 40], [40, 180, 60], [40, 60, 200], [220, 220, 220]];

    /// The pattern's colour at (x, y) as stored.
    pub fn stored(x: u32, y: u32) -> [u8; 3] {
        if y >= 320 {
            let v = (x as f32 * 255.0 / 479.0).round() as u8;
            return [v, v, v];
        }
        QUARTERS[(x >= 240) as usize + 2 * (y >= 160) as usize]
    }

    /// The fixture `name`, written into `dir`.
    pub fn put(dir: &Path, name: &str) -> PathBuf {
        let bytes: &[u8] = match name {
            "iphone.heic" => include_bytes!("../testdata/pictures/iphone.heic"),
            "plain.heic" => include_bytes!("../testdata/pictures/plain.heic"),
            "pattern.avif" => include_bytes!("../testdata/pictures/pattern.avif"),
            "pattern.jxl" => include_bytes!("../testdata/pictures/pattern.jxl"),
            "pattern.jp2" => include_bytes!("../testdata/pictures/pattern.jp2"),
            "ramp10.heic" => include_bytes!("../testdata/pictures/ramp10.heic"),
            "ramp10.avif" => include_bytes!("../testdata/pictures/ramp10.avif"),
            "hdr10.avif" => include_bytes!("../testdata/pictures/hdr10.avif"),
            "ramp16.jxl" => include_bytes!("../testdata/pictures/ramp16.jxl"),
            _ => panic!("no fixture {name}"),
        };
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    pub fn near(a: [u8; 3], b: [u8; 3], by: i32) -> bool {
        (0..3).all(|c| (a[c] as i32 - b[c] as i32).abs() <= by)
    }
}

#[cfg(test)]
mod copies {
    use super::pictures::{near, put, stored};
    use super::*;
    use crate::markup::{Area, Note, Shape, PEN_COLOURS};

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("spatiloupe-copies-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_heic_and_its_kin_are_copied_as_jpegs_that_open_as_they_were_shown() {
        let dir = folder("heic");
        // Each, as shown -- the iPhone's turned upright by its irot -- with where its grey quarter
        // is to draw a box in, and where its pixels as shown were stored.
        type Stored = fn(u32, u32) -> (u32, u32);
        let upright: Stored = |x, y| (x, y);
        // An iPhone's, stored on its side and turned a quarter clockwise to be shown.
        let iphone: Stored = |x, y| (y, 359 - x);
        // Which part of the pattern a stored pixel is in: a quarter, or the ramp.
        let part = |(x, y): (u32, u32)| if y >= 320 { 4 } else { (x >= 240) as u32 + 2 * (y >= 160) as u32 };
        let cases: [(&str, (u32, u32), Area, Stored); 5] = [
            ("iphone.heic", (360, 480), Area::new(30.0, 370.0, 120.0, 440.0), iphone),
            ("plain.heic", (480, 360), Area::new(280.0, 190.0, 440.0, 290.0), upright),
            ("pattern.avif", (480, 360), Area::new(280.0, 190.0, 440.0, 290.0), upright),
            ("pattern.jxl", (480, 360), Area::new(280.0, 190.0, 440.0, 290.0), upright),
            ("pattern.jp2", (480, 360), Area::new(280.0, 190.0, 440.0, 290.0), upright),
        ];
        for (name, (w, h), area, was) in cases {
            // Each in a folder of its own: pattern.avif and pattern.jxl would both be "pattern (edited).jpg".
            let own = dir.join(name.replace('.', "-"));
            std::fs::create_dir_all(&own).unwrap();
            let path = put(&own, name);
            let marks = Marks { notes: vec![Note::new(0, Shape::Box(area), PEN_COLOURS[10].colour, 6.0)], ..Marks::default() };
            // Not written over: only copied.
            let refused = save(&Saving { source: path.clone(), marks: marks.clone(), frame: (w as f32, h as f32), copy: false });
            assert!(refused.unwrap_err().contains("Save a copy"), "{name}");
            let copy = save(&Saving { source: path.clone(), marks, frame: (w as f32, h as f32), copy: true }).unwrap_or_else(|e| panic!("{name}: {e}"));
            let stem = name.rsplit_once('.').unwrap().0;
            assert_eq!(copy, own.join(format!("{stem} (edited).jpg")), "{name}");
            let read = image::open(&copy).unwrap().to_rgb8();
            assert_eq!(read.dimensions(), (w, h), "{name}: written as shown");
            // Its colours as they were shown, at points well inside each part -- the quarters and
            // the ramp -- through HEVC, AV1 and the rest and then JPEG.
            let mut checked = 0;
            for y in (12..h - 12).step_by(23) {
                for x in (12..w - 12).step_by(23) {
                    let inside = |px: u32, py: u32| (area.x0 - 6.0..area.x1 + 6.0).contains(&(px as f32)) && (area.y0 - 6.0..area.y1 + 6.0).contains(&(py as f32));
                    let around = [(x - 10, y - 10), (x + 10, y + 10), (x - 10, y + 10), (x + 10, y - 10)];
                    // Away from where the parts meet, and from the box.
                    if inside(x, y) || around.iter().any(|&(ax, ay)| part(was(ax, ay)) != part(was(x, y))) {
                        continue;
                    }
                    let (sx, sy) = was(x, y);
                    let got = read.get_pixel(x, y).0;
                    assert!(near(got, stored(sx, sy), 12), "{name} at ({x}, {y}): {got:?}, was {:?}", stored(sx, sy));
                    checked += 1;
                }
            }
            assert!(checked > 100, "{name}: {checked}");
            // The box: red on its left edge, the grey inside it untouched.
            let mid = ((area.y0 + area.y1) * 0.5) as u32;
            let edge = read.get_pixel(area.x0 as u32, mid).0;
            assert!(edge[0] > 180 && edge[1] < 110 && edge[2] < 110, "{name}: {edge:?}");
            let centre = read.get_pixel(((area.x0 + area.x1) * 0.5) as u32, mid).0;
            assert!(near(centre, [220, 220, 220], 12), "{name}: {centre:?}");
        }
        // The iPhone's: its Display P3 profile and its camera's account kept, upright and its size
        // as written.
        let original = crate::picture::heif_icc(&mut std::fs::File::open(dir.join("iphone-heic/iphone.heic")).unwrap()).expect("the HEIC's own profile");
        // An ICC profile ("acsp"), named Display P3 in UTF-16 as its description is.
        let described: Vec<u8> = "Display P3".encode_utf16().flat_map(u16::to_be_bytes).collect();
        assert!(&original[36..40] == b"acsp" && original.windows(described.len()).any(|w| w == described));
        let copy = dir.join("iphone-heic/iphone (edited).jpg");
        let mut decoder = image::codecs::jpeg::JpegDecoder::new(std::io::BufReader::new(std::fs::File::open(&copy).unwrap())).unwrap();
        assert_eq!(image::ImageDecoder::icc_profile(&mut decoder).unwrap(), Some(original), "the profile carried to the JPEG");
        let exif = exif::Reader::new().read_from_container(&mut std::io::BufReader::new(std::fs::File::open(&copy).unwrap())).unwrap();
        let get = |tag| exif.get_field(tag, exif::In::PRIMARY).map(|f| f.display_value().to_string());
        assert_eq!(get(exif::Tag::Orientation).as_deref(), Some("row 0 at top and column 0 at left"), "upright");
        assert_eq!(get(exif::Tag::Make).as_deref(), Some("\"Apple\""));
        assert_eq!(get(exif::Tag::DateTimeOriginal).as_deref(), Some("2026-11-19 10:00:00"));
        assert_eq!((get(exif::Tag::PixelXDimension), get(exif::Tag::PixelYDimension)), (Some("360".into()), Some("480".into())));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
