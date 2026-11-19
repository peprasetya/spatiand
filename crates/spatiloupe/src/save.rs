//! A picture marked up, written: drawn on at its own size, cropped, and written in its own
//! format where SpatiLoupe writes that format -- over the file, whole or not at all -- or as a
//! copy beside it. A PDF is written by `annotate`, on the PDF's own thread.

use std::path::PathBuf;

use image::{DynamicImage, ExtendedColorType, ImageEncoder, ImageFormat, RgbaImage};

use crate::ink::{self, Map};
use crate::markup::Marks;

/// How a JPEG is written: high, as a photograph written again should be.
const JPEG_QUALITY: u8 = 92;

/// The formats a picture is written back in as it is: what the image crate writes at eight bits
/// a channel without losing what the file had. JPEG is written again at high quality, WebP
/// without loss (the image crate's only WebP). Any other -- HEIC, AVIF, JPEG XL, JPEG 2000, TIFF,
/// GIF, OpenEXR, a drawing -- is saved as a copy.
pub fn in_place(name: &str) -> Option<ImageFormat> {
    let extension = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase())?;
    match extension.as_str() {
        "jpg" | "jpeg" | "jpe" | "jfif" => Some(ImageFormat::Jpeg),
        "png" => Some(ImageFormat::Png),
        "webp" => Some(ImageFormat::WebP),
        "bmp" => Some(ImageFormat::Bmp),
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

/// What a copy of a picture is written as: its own format where it can be, else JPEG -- or PNG,
/// for a picture with see-through parts, which JPEG has not.
pub fn copy_format(name: &str, clear: bool) -> (ImageFormat, Option<&'static str>) {
    match in_place(name) {
        Some(format) => (format, None),
        None if clear => (ImageFormat::Png, Some("png")),
        None => (ImageFormat::Jpeg, Some("jpg")),
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

/// `image` in `format`, with the file's colour profile and camera's account where the format
/// carries them.
fn encode(image: &RgbaImage, format: ImageFormat, exif: Option<Vec<u8>>, icc: Option<Vec<u8>>) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let (w, h) = image.dimensions();
    let opaque = image.pixels().all(|p| p.0[3] == 255);
    let rgb = || DynamicImage::ImageRgba8(image.clone()).to_rgb8().into_raw();
    // Without see-through parts, three channels: a third smaller, and what a photograph is.
    let (data, kind) = if opaque || format == ImageFormat::Jpeg { (rgb(), ExtendedColorType::Rgb8) } else { (image.as_raw().clone(), ExtendedColorType::Rgba8) };
    let exif = exif.map(|exif| exif_upright(exif, (w, h)));
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
    match format {
        ImageFormat::Jpeg => finish(image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY), &data, (w, h), kind, exif, icc)?,
        ImageFormat::Png => finish(image::codecs::png::PngEncoder::new(&mut out), &data, (w, h), kind, exif, icc)?,
        ImageFormat::WebP => finish(image::codecs::webp::WebPEncoder::new_lossless(&mut out), &data, (w, h), kind, exif, icc)?,
        ImageFormat::Bmp => finish(image::codecs::bmp::BmpEncoder::new(&mut out), &data, (w, h), kind, exif, icc)?,
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

/// The picture of `saving.source` at its own size, turned, drawn on and cropped as its marks
/// say -- what is written -- and, from the file, its camera's account and colour profile and the
/// format it is in.
pub fn render(saving: &Saving) -> Result<(RgbaImage, crate::picture::Whole), String> {
    let mut whole = crate::picture::read_whole(&saving.source)?;
    let image = std::mem::replace(&mut whole.image, RgbaImage::new(0, 0));
    let mut image = match saving.marks.turns % 4 {
        1 => image::imageops::rotate90(&image),
        2 => image::imageops::rotate180(&image),
        3 => image::imageops::rotate270(&image),
        _ => image,
    };
    let (w, h) = image.dimensions();
    // The marks' units to this picture's pixels: one to one but for a drawing, drawn at its
    // shown size rather than its own.
    let scale = w as f32 / saving.frame.0.max(1e-3);
    let notes: Vec<_> = saving.marks.notes.iter().filter(|n| n.page == 0).collect();
    ink::paint_over(&mut image, w, h, &notes, Map { scale, dx: 0.0, dy: 0.0 })?;
    if let Some(crop) = saving.marks.crop(0) {
        let x0 = (crop.x0 * scale).round().clamp(0.0, w as f32 - 1.0) as u32;
        let y0 = (crop.y0 * scale).round().clamp(0.0, h as f32 - 1.0) as u32;
        let x1 = ((crop.x1 * scale).round() as u32).clamp(x0 + 1, w);
        let y1 = ((crop.y1 * scale).round() as u32).clamp(y0 + 1, h);
        image = image::imageops::crop_imm(&image, x0, y0, x1 - x0, y1 - y0).to_image();
    }
    Ok((image, whole))
}

/// Write the picture marked up: over itself in its own format, or as a copy beside it. Where it
/// was written.
pub fn save(saving: &Saving) -> Result<PathBuf, String> {
    let name = crate::place::name(&saving.source);
    if !saving.copy && in_place(&name).is_none() {
        return Err(format!("SpatiLoupe does not write pictures like {name}: Save a copy writes a JPEG or PNG beside it"));
    }
    let (image, whole) = render(saving)?;
    let clear = image.pixels().any(|p| p.0[3] < 255);
    if saving.copy {
        let (format, extension) = copy_format(&name, clear);
        // A file named as one format and holding another is copied as what it holds is not:
        // what it is named, which is what it is written as.
        let keep = whole.format == Some(format);
        let (exif, icc) = if keep { (whole.exif, whole.icc) } else { (None, whole.icc) };
        let bytes = encode(&image, format, exif, icc)?;
        let folder = crate::place::parent(&saving.source).ok_or("it is in no folder")?;
        let copy = copy_name(&name, extension, |candidate| crate::place::exists(&crate::place::join(&folder, candidate)));
        let path = crate::place::join(&folder, &copy);
        crate::place::write_new(&path, &bytes)?;
        return Ok(path);
    }
    let format = in_place(&name).ok_or_else(|| format!("SpatiLoupe does not write pictures like {name}: Save a copy writes a JPEG or PNG beside it"))?;
    if whole.format != Some(format) {
        return Err(format!("{name} is not the kind of picture its name says: Save a copy writes it as one"));
    }
    let bytes = encode(&image, format, whole.exif, whole.icc)?;
    crate::place::replace(&saving.source, &bytes)?;
    Ok(saving.source.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markup::{Area, Ink, Note, Shape, Weight};

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
        assert_eq!(copy_name("trip.2024.pdf", None, none), "trip.2024 (edited).pdf");
    }

    #[test]
    fn which_pictures_are_written_back_as_they_are() {
        assert_eq!(in_place("a.JPG"), Some(ImageFormat::Jpeg));
        assert_eq!(in_place("a.jpeg"), Some(ImageFormat::Jpeg));
        assert_eq!(in_place("a.png"), Some(ImageFormat::Png));
        assert_eq!(in_place("a.webp"), Some(ImageFormat::WebP));
        assert_eq!(in_place("a.bmp"), Some(ImageFormat::Bmp));
        for other in ["a.heic", "a.avif", "a.jxl", "a.jp2", "a.exr", "a.hdr", "a.tif", "a.gif", "a.svg", "a"] {
            assert_eq!(in_place(other), None, "{other}");
        }
        assert_eq!(copy_format("a.heic", false), (ImageFormat::Jpeg, Some("jpg")));
        assert_eq!(copy_format("a.heic", true), (ImageFormat::Png, Some("png")));
        assert_eq!(copy_format("a.webp", true), (ImageFormat::WebP, None));
    }

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("spatiloupe-save-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn red_box(area: Area, size: f32) -> Note {
        Note { page: 0, shape: Shape::Box(area), ink: Ink::Red, weight: Weight::Medium, size }
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
            notes: vec![Note { page: 0, shape: Shape::Arrow { from: (5.0, 2.0), to: (5.0, 28.0) }, ink: Ink::Red, weight: Weight::Heavy, size: 2.0 }],
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
        let words = Note { page: 0, shape: Shape::Text { at: (10.0, 10.0), words: "Hello Привет 你好".into(), extent: (0.0, 0.0) }, ink: Ink::Blue, weight: Weight::Medium, size: 40.0 };
        let marks = Marks { notes: vec![words], ..Marks::default() };
        save(&Saving { source: path.clone(), marks, frame: (400.0, 120.0), copy: false }).unwrap();
        let read = image::open(&path).unwrap().to_rgba8();
        let blue = |x0: u32, x1: u32| (10..70).flat_map(|y| (x0..x1).map(move |x| (x, y))).filter(|&(x, y)| read.get_pixel(x, y).0[2] > 150 && read.get_pixel(x, y).0[0] < 120).count();
        // Latin, Cyrillic and Chinese each put ink where they are set.
        let (latin, _) = crate::ink::measure(&mut crate::ink::fonts(), "Hello ", 40.0);
        let (both, _) = crate::ink::measure(&mut crate::ink::fonts(), "Hello Привет ", 40.0);
        let (latin, both) = (10 + latin as u32, 10 + both as u32);
        assert!(blue(10, latin) > 100, "Latin");
        assert!(blue(latin, both) > 100, "Cyrillic");
        assert!(blue(both, 400) > 100, "Chinese");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
