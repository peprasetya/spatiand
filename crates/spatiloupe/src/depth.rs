//! A picture saved at its own depth (the owner, 2026-11-19, of pictures deeper than eight bits:
//! "can it be preserve?"). The marks are painted at eight bits -- tiny-skia's -- into a layer of
//! their own at the picture's size, and laid over the picture at its depth: where the layer is
//! clear the picture's own samples are kept to the last bit, and where it is not the marks'
//! colours are put on the picture's own curve first -- light itself for an OpenEXR's or a
//! Radiance HDR's floats (sRGB's curve taken off), PQ or HLG for an HDR picture's code values,
//! at the brightness of paper white -- so that a red box is the same red in each.
//!
//! And the formats that keep the depth, written as they were read: PNG at sixteen bits (with its
//! colours' code points, cICP, where a HEIF or AVIF gave those rather than a profile); TIFF by the
//! tiff crate, the image crate's own TIFF library, with its compression, its resolution, its
//! profile and its descriptive tags, where it holds nothing more that would be lost; OpenEXR by
//! the exr crate, every channel of its layer turned and cropped alike, its samples' types, its
//! compression and its attributes kept; Radiance HDR with its header's own lines.

use std::io::Cursor;

use image::DynamicImage;

use crate::picture::Whole;

/// The curve a picture's samples are on, which a mark's colour is put on before it is laid.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Curve {
    /// sRGB's, or near it: the marks' eight bits as they are, widened.
    Srgb,
    /// Light itself (floats): sRGB's curve taken off.
    Linear,
    /// SMPTE ST 2084 (an HDR10 picture's): light, paper white at 203 nits of 10,000.
    Pq,
    /// ARIB STD-B67 (HLG): paper white at its 75 per cent.
    Hlg,
}

impl Curve {
    /// A picture's curve: floats are light; a picture that names PQ or HLG (H.273's 16, 18) is
    /// on that; any other on sRGB's.
    pub fn of(whole: &Whole) -> Curve {
        if matches!(whole.image, DynamicImage::ImageRgb32F(_) | DynamicImage::ImageRgba32F(_)) {
            return Curve::Linear;
        }
        match whole.cicp {
            Some((_, 16)) => Curve::Pq,
            Some((_, 18)) => Curve::Hlg,
            _ => Curve::Srgb,
        }
    }

    /// A mark's colour, `c` of sRGB from 0 to 1, on this curve.
    pub fn put(self, c: f32) -> f32 {
        let light = || if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) };
        match self {
            Curve::Srgb => c,
            Curve::Linear => light(),
            Curve::Pq => {
                let (m1, m2, c1, c2, c3) = (0.159_301_76, 78.843_75, 0.835_937_5, 18.851_563, 18.6875);
                let y = (light() * 203.0 / 10_000.0).powf(m1);
                ((c1 + c2 * y) / (1.0 + c3 * y)).powf(m2)
            }
            Curve::Hlg => {
                // Paper white is HLG's 75 per cent: a scene light of 0.2652.
                let e = light() * 0.2652;
                let (a, b, cc) = (0.178_832_77, 0.284_668_92, 0.559_910_7);
                if e <= 1.0 / 12.0 { (3.0 * e).sqrt() } else { a * (12.0 * e - b).ln() + cc }
            }
        }
    }
}

/// Whether the layer of marks has anything in it.
pub fn any(layer: &[u8]) -> bool {
    layer.chunks_exact(4).any(|p| p[3] != 0)
}

/// The marks' `layer` (tiny-skia's eight bits, alpha multiplied in, at the picture's size) laid
/// over `image` at its own depth. A grey picture is made a coloured one of the same depth first
/// (the marks have colour); where the layer is clear nothing is touched.
pub fn lay(image: DynamicImage, layer: &[u8], curve: Curve) -> DynamicImage {
    if !any(layer) {
        return image;
    }
    let mut image = match image {
        DynamicImage::ImageLuma8(_) => DynamicImage::ImageRgb8(image.to_rgb8()),
        DynamicImage::ImageLumaA8(_) => DynamicImage::ImageRgba8(image.to_rgba8()),
        DynamicImage::ImageLuma16(_) => DynamicImage::ImageRgb16(image.to_rgb16()),
        DynamicImage::ImageLumaA16(_) => DynamicImage::ImageRgba16(image.to_rgba16()),
        DynamicImage::ImageRgb8(_) | DynamicImage::ImageRgba8(_) | DynamicImage::ImageRgb16(_) | DynamicImage::ImageRgba16(_) | DynamicImage::ImageRgb32F(_) | DynamicImage::ImageRgba32F(_) => image,
        other => DynamicImage::ImageRgba8(other.to_rgba8()),
    };
    let unit8 = |v: u8| v as f32 / 255.0;
    let unit16 = |v: u16| v as f32 / 65535.0;
    let to8 = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    let to16 = |v: f32| (v.clamp(0.0, 1.0) * 65535.0).round() as u16;
    match &mut image {
        DynamicImage::ImageRgb8(b) => over(b, 3, layer, unit8, to8, curve),
        DynamicImage::ImageRgba8(b) => over(b, 4, layer, unit8, to8, curve),
        DynamicImage::ImageRgb16(b) => over(b, 3, layer, unit16, to16, curve),
        DynamicImage::ImageRgba16(b) => over(b, 4, layer, unit16, to16, curve),
        // Light: as bright as it is, above 1 where it is.
        DynamicImage::ImageRgb32F(b) => over(b, 3, layer, |v| v, |v| v, curve),
        DynamicImage::ImageRgba32F(b) => over(b, 4, layer, |v| v, |v| v, curve),
        _ => {}
    }
    image
}

/// The layer over `samples`, `channels` a pixel, read and written by `get` and `set` (0 to 1).
fn over<T: Copy>(samples: &mut [T], channels: usize, layer: &[u8], get: impl Fn(T) -> f32, set: impl Fn(f32) -> T, curve: Curve) {
    for (px, mark) in samples.chunks_exact_mut(channels).zip(layer.chunks_exact(4)) {
        if mark[3] == 0 {
            continue;
        }
        let a = mark[3] as f32 / 255.0;
        // The mark's own colour, its alpha taken out, on the picture's curve.
        let ink = |k: usize| curve.put((mark[k] as f32 / 255.0 / a).min(1.0));
        if channels == 4 {
            let under = get(px[3]).clamp(0.0, 1.0);
            let alpha = a + under * (1.0 - a);
            for k in 0..3 {
                px[k] = set((ink(k) * a + get(px[k]) * under * (1.0 - a)) / alpha.max(1e-6));
            }
            px[3] = set(alpha);
        } else {
            for k in 0..3 {
                px[k] = set(ink(k) * a + get(px[k]) * (1.0 - a));
            }
        }
    }
}

/// A quarter turn `turns` times clockwise, of any picture.
pub fn turned(image: DynamicImage, turns: u8) -> DynamicImage {
    match turns % 4 {
        1 => image.rotate90(),
        2 => image.rotate180(),
        3 => image.rotate270(),
        _ => image,
    }
}

// --- PNG's code points ---

/// A PNG with a cICP chunk after its header: the colours' primaries and transfer as a HEIF or AVIF
/// named them (ITU-T H.273), RGB, full range -- which keeps an HDR picture HDR, and a wide one wide.
pub fn with_cicp(png: Vec<u8>, (primaries, transfer): (u8, u8)) -> Vec<u8> {
    // The signature (8) and IHDR (25): the chunk goes straight after.
    if png.len() < 33 || &png[12..16] != b"IHDR" {
        return png;
    }
    let body = [primaries, transfer, 0, 1];
    let mut chunk = 4u32.to_be_bytes().to_vec();
    chunk.extend_from_slice(b"cICP");
    chunk.extend_from_slice(&body);
    chunk.extend_from_slice(&crc32(&chunk[4..]).to_be_bytes());
    let mut out = png[..33].to_vec();
    out.extend_from_slice(&chunk);
    out.extend_from_slice(&png[33..]);
    out
}

/// PNG's CRC-32 (ISO 3309), of what follows a chunk's length.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in bytes {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

// --- TIFF, as it was read ---

/// What of a TIFF is written back as it was: how it is compressed, its predictor, and the tags it
/// holds besides its pixels -- its resolution, its profile, its XMP and its words.
pub struct TiffAsRead {
    compression: u16,
    predictor: u16,
    resolution: Option<(tiff::encoder::Rational, tiff::encoder::Rational, u16)>,
    bytes: Vec<(tiff::tags::Tag, Vec<u8>)>,
    words: Vec<(tiff::tags::Tag, String)>,
}

/// The tags that are its pixels' own (written again by the encoder for what is written), and those
/// carried over as they were. Any other -- Exif's, GPS's, IPTC, Photoshop's layers, sub-images --
/// would be lost, and the TIFF is only copied.
const TIFF_PIXELS: [u16; 15] = [254, 256, 257, 258, 259, 262, 273, 274, 277, 278, 279, 284, 317, 338, 339];
const TIFF_CARRIED: [u16; 14] = [269, 270, 271, 272, 282, 283, 285, 296, 305, 306, 315, 316, 700, 33432];

/// Whether SpatiLoupe writes this TIFF back as it was read, and how; or why not.
pub fn tiff_as_read(bytes: &[u8]) -> Result<TiffAsRead, String> {
    use tiff::decoder::{ifd::Value, Decoder};
    use tiff::tags::Tag;
    let mut d = Decoder::new(Cursor::new(bytes)).map_err(|e| e.to_string())?;
    if d.more_images() {
        return Err("it holds more than one picture".into());
    }
    let colours = d.colortype().map_err(|e| e.to_string())?;
    let float = d.find_tag_unsigned::<u16>(Tag::SampleFormat).ok().flatten() == Some(3);
    let fine = match colours {
        tiff::ColorType::Gray(8 | 16) | tiff::ColorType::GrayA(8 | 16) | tiff::ColorType::RGB(8 | 16) | tiff::ColorType::RGBA(8 | 16) => !float,
        tiff::ColorType::RGB(32) | tiff::ColorType::RGBA(32) => float,
        _ => false,
    };
    if !fine {
        return Err("its colours are of a kind SpatiLoupe does not write".into());
    }
    if d.find_tag_unsigned::<u16>(Tag::PlanarConfiguration).ok().flatten().unwrap_or(1) != 1 {
        return Err("its colours are stored apart".into());
    }
    let compression = d.find_tag_unsigned::<u16>(Tag::Compression).ok().flatten().unwrap_or(1);
    if !matches!(compression, 1 | 5 | 8 | 32946 | 32773) {
        return Err("it is compressed in a way SpatiLoupe does not write".into());
    }
    let predictor = d.find_tag_unsigned::<u16>(Tag::Predictor).ok().flatten().unwrap_or(1);
    if predictor > 2 {
        return Err("it is compressed in a way SpatiLoupe does not write".into());
    }
    // Every tag it has: its pixels', or one carried, or it is not written over.
    let ifd = d.ifd_pointer().ok_or("no directory")?;
    let directory = d.read_directory(ifd).map_err(|e| e.to_string())?;
    let mut asread = TiffAsRead { compression, predictor, resolution: None, bytes: Vec::new(), words: Vec::new() };
    for (tag, _) in directory.iter() {
        let n = tag.to_u16();
        if !(TIFF_PIXELS.contains(&n) || TIFF_CARRIED.contains(&n) || n == 34675) {
            return Err("it holds more than SpatiLoupe writes back (its camera's account, or layers)".into());
        }
    }
    let rational = |v: Value| match v {
        Value::Rational(n, d) => Some(tiff::encoder::Rational { n, d }),
        Value::List(l) => match l.first() {
            Some(Value::Rational(n, d)) => Some(tiff::encoder::Rational { n: *n, d: *d }),
            _ => None,
        },
        _ => None,
    };
    if let (Some(x), Some(y)) = (d.find_tag(Tag::XResolution).ok().flatten().and_then(rational), d.find_tag(Tag::YResolution).ok().flatten().and_then(rational)) {
        let unit = d.find_tag_unsigned::<u16>(Tag::ResolutionUnit).ok().flatten().unwrap_or(2);
        asread.resolution = Some((x, y, unit));
    }
    for n in [34675u16, 700] {
        let tag = Tag::from_u16_exhaustive(n);
        if let Ok(Some(value)) = d.find_tag(tag) {
            if let Ok(v) = value.into_u8_vec() {
                asread.bytes.push((tag, v));
            }
        }
    }
    for n in [269u16, 270, 271, 272, 285, 305, 306, 315, 316, 33432] {
        let tag = Tag::from_u16_exhaustive(n);
        if let Ok(Some(value)) = d.find_tag(tag) {
            if let Ok(text) = value.into_string() {
                asread.words.push((tag, text));
            }
        }
    }
    Ok(asread)
}

/// `image` written as a TIFF the way `asread` says the original was.
pub fn tiff_bytes(image: &DynamicImage, asread: &TiffAsRead) -> Result<Vec<u8>, String> {
    use tiff::encoder::{colortype, Compression, DeflateLevel, Predictor, TiffEncoder};
    let mut out = Cursor::new(Vec::new());
    let compression = match asread.compression {
        5 => Compression::Lzw,
        8 | 32946 => Compression::Deflate(DeflateLevel::Balanced),
        32773 => Compression::Packbits,
        _ => Compression::Uncompressed,
    };
    let predictor = if asread.predictor == 2 { Predictor::Horizontal } else { Predictor::None };
    let mut encoder = TiffEncoder::new(&mut out).map_err(|e| e.to_string())?.with_compression(compression).with_predictor(predictor);
    let (w, h) = (image.width(), image.height());
    macro_rules! write {
        ($kind:ty, $data:expr) => {{
            let mut tiff = encoder.new_image::<$kind>(w, h).map_err(|e| e.to_string())?;
            if let Some((x, y, unit)) = &asread.resolution {
                tiff.x_resolution(x.clone());
                tiff.y_resolution(y.clone());
                tiff.resolution_unit(tiff::tags::ResolutionUnit::from_u16(*unit).unwrap_or(tiff::tags::ResolutionUnit::Inch));
            }
            for (tag, value) in &asread.bytes {
                tiff.encoder().write_tag(*tag, &value[..]).map_err(|e| e.to_string())?;
            }
            for (tag, text) in &asread.words {
                tiff.encoder().write_tag(*tag, &text[..]).map_err(|e| e.to_string())?;
            }
            tiff.write_data($data).map_err(|e| e.to_string())?;
        }};
    }
    match image {
        DynamicImage::ImageLuma8(b) => write!(colortype::Gray8, b.as_raw()),
        DynamicImage::ImageRgb8(b) => write!(colortype::RGB8, b.as_raw()),
        DynamicImage::ImageRgba8(b) => write!(colortype::RGBA8, b.as_raw()),
        DynamicImage::ImageLuma16(b) => write!(colortype::Gray16, b.as_raw()),
        DynamicImage::ImageRgb16(b) => write!(colortype::RGB16, b.as_raw()),
        DynamicImage::ImageRgba16(b) => write!(colortype::RGBA16, b.as_raw()),
        DynamicImage::ImageRgb32F(b) => write!(colortype::RGB32Float, b.as_raw()),
        DynamicImage::ImageRgba32F(b) => write!(colortype::RGBA32Float, b.as_raw()),
        // Grey with alpha: as coloured, which TIFF's encoder writes.
        DynamicImage::ImageLumaA8(_) => write!(colortype::RGBA8, image.to_rgba8().as_raw()),
        DynamicImage::ImageLumaA16(_) => write!(colortype::RGBA16, image.to_rgba16().as_raw()),
        other => write!(colortype::RGBA8, other.to_rgba8().as_raw()),
    }
    Ok(out.into_inner())
}

// --- OpenEXR, as it was read ---

/// Whether SpatiLoupe writes this OpenEXR back as it was read: one layer, flat (no deep samples),
/// at one resolution, its colours red, green and blue at every pixel, what is seen of it all there
/// is of it (its display window its data's); or why not.
pub fn exr_as_read(bytes: &[u8]) -> Result<(), String> {
    use exr::meta::{attribute::LevelMode, BlockDescription, MetaData};
    let meta = MetaData::read_from_buffered(Cursor::new(bytes), false).map_err(|e| e.to_string())?;
    let [header] = &meta.headers[..] else { return Err("it holds more than one layer".into()) };
    if header.deep {
        return Err("it holds deep samples".into());
    }
    if let BlockDescription::Tiles(tiles) = header.blocks {
        if tiles.level_mode != LevelMode::Singular {
            return Err("it holds smaller copies of itself (mip maps)".into());
        }
    }
    let names: Vec<String> = header.channels.list.iter().map(|c| c.name.to_string()).collect();
    if !["R", "G", "B"].iter().all(|n| names.iter().any(|m| m == n)) {
        return Err("its colours are not red, green and blue".into());
    }
    if header.channels.list.iter().any(|c| c.sampling.x() != 1 || c.sampling.y() != 1) {
        return Err("some of its channels are not at every pixel".into());
    }
    let window = header.shared_attributes.display_window;
    if window.position.x() != header.own_attributes.layer_position.x() || window.position.y() != header.own_attributes.layer_position.y() || window.size.x() != header.layer_size.x() || window.size.y() != header.layer_size.y() {
        return Err("what is shown of it is not all there is of it".into());
    }
    Ok(())
}

/// The OpenEXR of `bytes` again: every channel of its layer turned `turns` quarters and cropped
/// to `crop` (x, y, w, h in the turned picture's pixels), the marks' `layer` (at the turned
/// picture's size) laid over its red, green, blue and alpha as light; its channels' sample types,
/// its compression, its blocks and its attributes as they were.
pub fn exr_bytes(bytes: &[u8], turns: u8, layer: &[u8], crop: Option<(u32, u32, u32, u32)>) -> Result<Vec<u8>, String> {
    use exr::image::FlatSamples;
    use exr::math::Vec2;
    use exr::prelude::*;
    let mut image = read().no_deep_data().largest_resolution_level().all_channels().all_layers().all_attributes().from_buffered(Cursor::new(bytes)).map_err(|e| e.to_string())?;
    let layer_data = image.layer_data.first_mut().ok_or("no layer")?;
    let (w, h) = (layer_data.size.x(), layer_data.size.y());
    let (tw, th) = if turns % 2 == 1 { (h, w) } else { (w, h) };
    let (cx, cy, cw, ch) = crop.map_or((0, 0, tw, th), |(x, y, cw, ch)| (x as usize, y as usize, cw as usize, ch as usize));
    // Where each pixel of the turned picture was in the stored one.
    let from = |x: usize, y: usize| -> usize {
        let (sx, sy) = match turns % 4 {
            1 => (y, h - 1 - x),
            2 => (w - 1 - x, h - 1 - y),
            3 => (w - 1 - y, x),
            _ => (x, y),
        };
        sy * w + sx
    };
    let samples = |data: &FlatSamples| -> Vec<f32> {
        match data {
            FlatSamples::F16(v) => v.iter().map(|s| s.to_f32()).collect(),
            FlatSamples::F32(v) => v.clone(),
            FlatSamples::U32(v) => v.iter().map(|&s| s as f32).collect(),
        }
    };
    let mut planes: Vec<Vec<f32>> = layer_data.channel_data.list.iter().map(|c| samples(&c.sample_data)).collect();
    // Turned, each.
    for plane in planes.iter_mut() {
        let turned: Vec<f32> = (0..th).flat_map(|y| (0..tw).map(move |x| (x, y))).map(|(x, y)| plane[from(x, y)]).collect();
        *plane = turned;
    }
    // The marks, over red, green, blue (and alpha), as light.
    let index = |name: &str| layer_data.channel_data.list.iter().position(|c| c.name.to_string() == name);
    let (r, g, b, a) = (index("R").ok_or("no red")?, index("G").ok_or("no green")?, index("B").ok_or("no blue")?, index("A"));
    if layer.len() == tw * th * 4 {
        for (i, mark) in layer.chunks_exact(4).enumerate() {
            if mark[3] == 0 {
                continue;
            }
            let alpha = mark[3] as f32 / 255.0;
            let ink = |k: usize| Curve::Linear.put((mark[k] as f32 / 255.0 / alpha).min(1.0));
            let under = a.map_or(1.0, |a| planes[a][i].clamp(0.0, 1.0));
            let out = alpha + under * (1.0 - alpha);
            for (k, c) in [r, g, b].into_iter().enumerate() {
                planes[c][i] = (ink(k) * alpha + planes[c][i] * under * (1.0 - alpha)) / out.max(1e-6);
            }
            if let Some(a) = a {
                planes[a][i] = out;
            }
        }
    }
    // Cropped, each, and put back as the type it was.
    for (channel, plane) in layer_data.channel_data.list.iter_mut().zip(planes) {
        let kept: Vec<f32> = (cy..cy + ch).flat_map(|y| (cx..cx + cw).map(move |x| (x, y))).map(|(x, y)| plane[y * tw + x]).collect();
        channel.sample_data = match &channel.sample_data {
            FlatSamples::F16(_) => FlatSamples::F16(kept.iter().map(|&v| f16::from_f32(v)).collect()),
            FlatSamples::F32(_) => FlatSamples::F32(kept),
            FlatSamples::U32(_) => FlatSamples::U32(kept.iter().map(|&v| v.round().max(0.0) as u32).collect()),
        };
    }
    layer_data.size = Vec2(cw, ch);
    let origin = image.attributes.display_window.position;
    layer_data.attributes.layer_position = origin;
    image.attributes.display_window = exr::meta::attribute::IntegerBounds { position: origin, size: Vec2(cw, ch) };
    let mut out = Cursor::new(Vec::new());
    image.write().to_buffered(&mut out).map_err(|e| e.to_string())?;
    Ok(out.into_inner())
}

// --- Radiance HDR, with its own header ---

/// Whether SpatiLoupe writes this Radiance HDR back as it was read: in RGBE, not XYZE.
pub fn hdr_as_read(bytes: &[u8]) -> Result<(), String> {
    let head = hdr_head(bytes).ok_or("not a Radiance picture")?;
    if head.iter().any(|line| line.trim().eq_ignore_ascii_case("FORMAT=32-bit_rle_xyze")) {
        return Err("its colours are in XYZ".into());
    }
    Ok(())
}

/// The lines of a Radiance HDR's header, its first ("#?RADIANCE") and its blank last left out.
fn hdr_head(bytes: &[u8]) -> Option<Vec<String>> {
    let end = bytes.windows(2).position(|w| w == b"\n\n")?;
    let text = std::str::from_utf8(&bytes[..end]).ok()?;
    let mut lines = text.lines();
    if !lines.next()?.starts_with("#?") {
        return None;
    }
    Some(lines.map(str::to_string).collect())
}

/// `image` as a Radiance HDR, its header the original's own -- its exposure, its primaries, its
/// software -- with the size written again.
pub fn hdr_bytes(image: &DynamicImage, original: &[u8]) -> Result<Vec<u8>, String> {
    let rgb = image.to_rgb32f();
    let (w, h) = rgb.dimensions();
    let mut out = Vec::new();
    let pixels: Vec<image::Rgb<f32>> = rgb.pixels().copied().collect();
    image::codecs::hdr::HdrEncoder::new(&mut out).encode(&pixels, w as usize, h as usize).map_err(|e| e.to_string())?;
    let Some(own) = hdr_head(original) else { return Ok(out) };
    // The encoder's own header ends at its blank line; the original's lines in its place.
    let Some(end) = out.windows(2).position(|w| w == b"\n\n") else { return Ok(out) };
    let mut head = b"#?RADIANCE\n".to_vec();
    for line in own {
        head.extend_from_slice(line.as_bytes());
        head.push(b'\n');
    }
    if !head.windows(7).any(|w| w == b"FORMAT=") {
        head.extend_from_slice(b"FORMAT=32-bit_rle_rgbe\n");
    }
    head.extend_from_slice(&out[end + 1..]);
    Ok(head)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markup::{Area, Ink, Marks, Note, Shape, Weight};
    use crate::save::{save, Saving};
    use std::path::PathBuf;

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("spatiloupe-depth-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A red box, its line 4 wide, round `area`.
    fn boxed(area: Area) -> Marks {
        Marks { notes: vec![Note { page: 0, shape: Shape::Box(area), ink: Ink::Red, weight: Weight::Medium, size: 4.0 }], ..Marks::default() }
    }

    /// Whether (x, y) is clear of a box's line round `area` (4 wide, and its soft edge).
    fn clear_of(area: Area, x: u32, y: u32) -> bool {
        let (x, y) = (x as f32 + 0.5, y as f32 + 0.5);
        let near = |v: f32, edge: f32| (v - edge).abs() < 4.0;
        let within = |v: f32, a: f32, b: f32| v > a - 4.0 && v < b + 4.0;
        !((near(x, area.x0) || near(x, area.x1)) && within(y, area.y0, area.y1) || (near(y, area.y0) || near(y, area.y1)) && within(x, area.x0, area.x1))
    }

    /// A 16-bit colour gradient whose every sample has low bits of its own: no 8-bit picture
    /// could hold it.
    fn deep(x: u32, y: u32) -> [u16; 3] {
        [(x * 997 + y * 13) as u16, (40_000 - x * 211 + y * 7) as u16, (x * y * 3 + 5) as u16]
    }

    #[test]
    fn a_sixteen_bit_png_keeps_its_low_bits_outside_the_marks() {
        let dir = folder("png16");
        let path = dir.join("deep.png");
        let image = image::ImageBuffer::<image::Rgb<u16>, Vec<u16>>::from_fn(64, 48, |x, y| image::Rgb(deep(x, y)));
        image.save(&path).unwrap();
        let area = Area::new(20.0, 12.0, 44.0, 36.0);
        save(&Saving { source: path.clone(), marks: boxed(area), frame: (64.0, 48.0), copy: false }).unwrap();
        let read = image::open(&path).unwrap();
        let DynamicImage::ImageRgb16(read) = read else { panic!("written at sixteen bits: {:?}", read.color()) };
        let mut kept = 0;
        for y in 0..48 {
            for x in 0..64 {
                if clear_of(area, x, y) {
                    assert_eq!(read.get_pixel(x, y).0, deep(x, y), "({x}, {y})");
                    kept += 1;
                }
            }
        }
        assert!(kept > 2000, "{kept}");
        // On the line, red: the mark's own eight bits, widened.
        assert_eq!(read.get_pixel(20, 24).0, [235 * 257, 51 * 257, 36 * 257]);
        // Turned a quarter and cropped, as a copy: its samples where the turn took them, still all
        // sixteen bits of them.
        let marks = Marks { turns: 1, crops: vec![(0, Area::new(8.0, 4.0, 40.0, 60.0))], ..Marks::default() };
        std::fs::write(&path, {
            let mut out = Vec::new();
            image.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
            out
        })
        .unwrap();
        let copy = save(&Saving { source: path.clone(), marks, frame: (48.0, 64.0), copy: true }).unwrap();
        let DynamicImage::ImageRgb16(read) = image::open(&copy).unwrap() else { panic!("sixteen bits") };
        assert_eq!(read.dimensions(), (32, 56));
        // Shown (x, y) of the turned picture was stored (y, 47 - x); cropped from (8, 4).
        for (x, y) in [(0u32, 0u32), (31, 55), (10, 20)] {
            let (tx, ty) = (x + 8, y + 4);
            assert_eq!(read.get_pixel(x, y).0, deep(ty, 47 - tx), "({x}, {y})");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_grey_sixteen_bit_png_marked_is_coloured_at_sixteen_bits() {
        let dir = folder("grey16");
        let path = dir.join("grey.png");
        image::ImageBuffer::<image::Luma<u16>, Vec<u16>>::from_fn(40, 30, |x, y| image::Luma([(x * 1601 + y * 3) as u16])).save(&path).unwrap();
        let area = Area::new(10.0, 8.0, 30.0, 22.0);
        save(&Saving { source: path.clone(), marks: boxed(area), frame: (40.0, 30.0), copy: false }).unwrap();
        let DynamicImage::ImageRgb16(read) = image::open(&path).unwrap() else { panic!("coloured, at sixteen bits") };
        let v = (2 * 1601 + 3) as u16;
        assert_eq!(read.get_pixel(2, 1).0, [v, v, v]);
        assert_eq!(read.get_pixel(10, 15).0, [235 * 257, 51 * 257, 36 * 257]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An OpenEXR as a renderer writes one: red, green and blue in half floats, brighter than
    /// white in places, a depth channel (Z) in full floats beside them, ZIP-compressed.
    fn exr(path: &std::path::Path, w: usize, h: usize) {
        use exr::prelude::*;
        let half = |f: &dyn Fn(usize, usize) -> f32| FlatSamples::F16((0..w * h).map(|i| f16::from_f32(f(i % w, i / w))).collect());
        let channels = AnyChannels::sort(SmallVec::from_vec(vec![
            AnyChannel::new("R", half(&|x, _| 0.25 + x as f32 * 0.25)),
            AnyChannel::new("G", half(&|_, y| 0.5 + y as f32 * 0.125)),
            AnyChannel::new("B", half(&|x, y| (x + y) as f32 * 0.0625)),
            AnyChannel::new("Z", FlatSamples::F32((0..w * h).map(|i| 1000.0 + i as f32).collect())),
        ]));
        let layer = Layer::new((w, h), LayerAttributes::named("main"), Encoding { compression: Compression::ZIP16, ..Encoding::default() }, channels);
        let mut image = Image::from_layer(layer);
        image.attributes.other.insert(Text::from("spatiloupeTest"), AttributeValue::Text(Text::from("SpatiLoupe's test")));
        image.write().to_file(path).unwrap();
    }

    #[test]
    fn an_openexr_keeps_its_light_above_one_and_its_other_channels_and_its_marks_are_the_same_red() {
        let dir = folder("exr");
        let path = dir.join("render.exr");
        let (w, h) = (32usize, 24usize);
        exr(&path, w, h);
        assert_eq!(crate::save::not_over("render.exr", &std::fs::read(&path).unwrap()), None, "written as it was read");
        let area = Area::new(8.0, 6.0, 24.0, 18.0);
        save(&Saving { source: path.clone(), marks: boxed(area), frame: (w as f32, h as f32), copy: false }).unwrap();
        use exr::prelude::*;
        let back = read().no_deep_data().largest_resolution_level().all_channels().all_layers().all_attributes().from_file(&path).unwrap();
        let layer = &back.layer_data[0];
        assert_eq!(layer.encoding.compression, Compression::ZIP16, "its compression kept");
        // (A single layer's own attributes are read as the layer's.)
        let own = back.attributes.other.get(&Text::from("spatiloupeTest")).or(layer.attributes.other.get(&Text::from("spatiloupeTest")));
        assert_eq!(own, Some(&AttributeValue::Text(Text::from("SpatiLoupe's test"))), "its attributes kept");
        let channel = |name: &str| layer.channel_data.list.iter().find(|c| c.name == Text::from(name)).unwrap();
        let get = |name: &str, x: usize, y: usize| match &channel(name).sample_data {
            FlatSamples::F16(v) => v[y * w + x].to_f32(),
            FlatSamples::F32(v) => v[y * w + x],
            FlatSamples::U32(v) => v[y * w + x] as f32,
        };
        assert!(matches!(channel("R").sample_data, FlatSamples::F16(_)) && matches!(channel("Z").sample_data, FlatSamples::F32(_)), "their types kept");
        // Outside the marks, the light as it was -- to 7.75, far above white -- and Z untouched.
        for y in 0..h {
            for x in 0..w {
                assert_eq!(get("Z", x, y), 1000.0 + (y * w + x) as f32);
                if clear_of(area, x as u32, y as u32) {
                    assert_eq!(get("R", x, y), f16::from_f32(0.25 + x as f32 * 0.25).to_f32(), "({x}, {y})");
                    assert_eq!(get("B", x, y), f16::from_f32((x + y) as f32 * 0.0625).to_f32());
                }
            }
        }
        assert!(get("R", 31, 0) > 7.0);
        // The box's red, as light: sRGB's 235, 51 and 36 with its curve taken off.
        let light = |c: f32| Curve::Linear.put(c / 255.0);
        let (r, g, b) = (get("R", 8, 12), get("G", 8, 12), get("B", 8, 12));
        assert!((r - light(235.0)).abs() < 0.003 && (g - light(51.0)).abs() < 0.001 && (b - light(36.0)).abs() < 0.001, "{r} {g} {b}");
        // And the image crate reads it as SpatiLoupe shows it.
        assert!(matches!(image::open(&path).unwrap(), DynamicImage::ImageRgb32F(_) | DynamicImage::ImageRgba32F(_)));
        // Turned and cropped as a copy: every channel alike.
        exr(&path, w, h);
        let marks = Marks { turns: 1, crops: vec![(0, Area::new(2.0, 4.0, 20.0, 30.0))], ..Marks::default() };
        let copy = save(&Saving { source: path.clone(), marks, frame: (h as f32, w as f32), copy: true }).unwrap();
        assert_eq!(copy, dir.join("render (edited).exr"));
        let again = read().no_deep_data().largest_resolution_level().all_channels().all_layers().all_attributes().from_file(&copy).unwrap();
        let layer = &again.layer_data[0];
        assert_eq!((layer.size.x(), layer.size.y()), (18, 26));
        let z = |x: usize, y: usize| match &layer.channel_data.list.iter().find(|c| c.name == Text::from("Z")).unwrap().sample_data {
            FlatSamples::F32(v) => v[y * 18 + x],
            _ => panic!("Z in floats"),
        };
        // Shown (x, y) turned was stored (y, 23 - x); cropped from (2, 4).
        for (x, y) in [(0usize, 0usize), (17, 25), (5, 9)] {
            let (tx, ty) = (x + 2, y + 4);
            assert_eq!(z(x, y), 1000.0 + ((23 - tx) * w + ty) as f32, "({x}, {y})");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_radiance_hdr_keeps_its_light_and_its_header() {
        let dir = folder("hdr");
        let path = dir.join("sky.hdr");
        let (w, h) = (24u32, 16u32);
        let light = |x: u32, y: u32| image::Rgb([0.5 + x as f32 * 0.5, 2.0 + y as f32, 0.125]);
        let pixels: Vec<image::Rgb<f32>> = (0..h).flat_map(|y| (0..w).map(move |x| light(x, y))).collect();
        let mut plain = Vec::new();
        image::codecs::hdr::HdrEncoder::new(&mut plain).encode(&pixels, w as usize, h as usize).unwrap();
        // Its own header, as a renderer's: an exposure and its software.
        let end = plain.windows(2).position(|w| w == b"\n\n").unwrap();
        let mut file = b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\nEXPOSURE=2.0\nSOFTWARE=SpatiLoupe's test\n".to_vec();
        file.extend_from_slice(&plain[end + 1..]);
        std::fs::write(&path, &file).unwrap();
        let area = Area::new(6.0, 4.0, 18.0, 12.0);
        save(&Saving { source: path.clone(), marks: boxed(area), frame: (w as f32, h as f32), copy: false }).unwrap();
        let written = std::fs::read(&path).unwrap();
        let head = String::from_utf8_lossy(&written[..written.windows(2).position(|w| w == b"\n\n").unwrap()]).into_owned();
        assert!(head.contains("EXPOSURE=2.0") && head.contains("SOFTWARE=SpatiLoupe's test") && head.contains("FORMAT=32-bit_rle_rgbe"), "{head}");
        let DynamicImage::ImageRgb32F(read) = image::open(&path).unwrap() else { panic!("floats") };
        for (x, y) in [(0u32, 0u32), (23, 15), (12, 8)] {
            let (got, was) = (read.get_pixel(x, y).0, light(x, y).0);
            assert!((0..3).all(|c| (got[c] - was[c]).abs() <= was[c] * 0.01), "({x}, {y}) {got:?} {was:?}");
        }
        let red = read.get_pixel(6, 8).0;
        assert!((red[0] - Curve::Linear.put(235.0 / 255.0)).abs() < 0.01 && red[1] < 0.05, "{red:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A 16-bit TIFF as a scanner writes one: LZW, 300 dots an inch, its software named.
    fn scan(path: &std::path::Path, pages: usize) {
        use tiff::encoder::{colortype::RGB16, Compression, Rational, TiffEncoder};
        let mut file = std::fs::File::create(path).unwrap();
        let mut encoder = TiffEncoder::new(&mut file).unwrap().with_compression(Compression::Lzw);
        let data: Vec<u16> = (0..30).flat_map(|y| (0..40).flat_map(move |x| deep(x, y))).collect();
        for _ in 0..pages {
            let mut image = encoder.new_image::<RGB16>(40, 30).unwrap();
            image.resolution(tiff::tags::ResolutionUnit::Inch, Rational { n: 300, d: 1 });
            image.encoder().write_tag(tiff::tags::Tag::Software, "SpatiLoupe's test").unwrap();
            image.write_data(&data).unwrap();
        }
    }

    #[test]
    fn a_sixteen_bit_tiff_is_written_over_as_it_was_read_or_copied_at_its_depth() {
        let dir = folder("tiff");
        let path = dir.join("scan.tif");
        scan(&path, 1);
        let area = Area::new(10.0, 8.0, 30.0, 22.0);
        save(&Saving { source: path.clone(), marks: boxed(area), frame: (40.0, 30.0), copy: false }).unwrap();
        let mut d = tiff::decoder::Decoder::new(std::fs::File::open(&path).unwrap()).unwrap();
        use tiff::tags::Tag;
        assert_eq!(d.get_tag_u32(Tag::Compression).unwrap(), 5, "LZW, as it was");
        assert_eq!(d.get_tag(Tag::XResolution).unwrap().into_u32_vec().ok(), Some(vec![300, 1]));
        assert_eq!(d.get_tag(Tag::Software).unwrap().into_string().unwrap(), "SpatiLoupe's test");
        let DynamicImage::ImageRgb16(read) = image::open(&path).unwrap() else { panic!("sixteen bits") };
        assert_eq!(read.get_pixel(1, 1).0, deep(1, 1));
        assert_eq!(read.get_pixel(39, 29).0, deep(39, 29));
        assert_eq!(read.get_pixel(10, 15).0, [235 * 257, 51 * 257, 36 * 257]);
        // Two pages: not written over (the second would be lost); its copy a 16-bit PNG.
        let pages = dir.join("pages.tif");
        scan(&pages, 2);
        let why = crate::save::not_over("pages.tif", &std::fs::read(&pages).unwrap());
        assert_eq!(why.as_deref(), Some("it holds more than one picture"));
        assert!(save(&Saving { source: pages.clone(), marks: boxed(area), frame: (40.0, 30.0), copy: false }).unwrap_err().contains("Save a copy"));
        let copy = save(&Saving { source: pages.clone(), marks: boxed(area), frame: (40.0, 30.0), copy: true }).unwrap();
        assert_eq!(copy, dir.join("pages (edited).png"));
        let DynamicImage::ImageRgb16(read) = image::open(&copy).unwrap() else { panic!("sixteen bits") };
        assert_eq!(read.get_pixel(1, 1).0, deep(1, 1));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_ten_bit_heic_or_avif_or_a_sixteen_bit_jpeg_xl_is_copied_as_a_sixteen_bit_png() {
        let dir = folder("deepcopies");
        for name in ["ramp10.heic", "ramp10.avif", "ramp16.jxl", "hdr10.avif"] {
            // Each in a folder of its own: two are "ramp10 (edited).png".
            let own = dir.join(name.replace('.', "-"));
            std::fs::create_dir_all(&own).unwrap();
            let path = crate::save::pictures::put(&own, name);
            let source = spatiand_media::still::whole_file(&path).unwrap();
            assert!(source.bits > 8, "{name}");
            let area = Area::new(100.0, 100.0, 300.0, 250.0);
            let copy = save(&Saving { source: path.clone(), marks: boxed(area), frame: (480.0, 360.0), copy: true }).unwrap_or_else(|e| panic!("{name}: {e}"));
            let stem = name.rsplit_once('.').unwrap().0;
            assert_eq!(copy, own.join(format!("{stem} (edited).png")), "{name}");
            let DynamicImage::ImageRgb16(read) = image::open(&copy).unwrap() else { panic!("{name}: sixteen bits") };
            // Outside the box, the samples FFmpeg gives at sixteen bits, as they are: more than
            // eight bits' worth (most not a multiple of 257).
            let (mut kept, mut fine) = (0, 0);
            for y in (0..360).step_by(7) {
                for x in (0..480).step_by(7) {
                    if !clear_of(area, x, y) {
                        continue;
                    }
                    let i = ((y * 480 + x) * 4) as usize;
                    let got = read.get_pixel(x, y).0;
                    assert_eq!(got, [source.rgba[i], source.rgba[i + 1], source.rgba[i + 2]], "{name} ({x}, {y})");
                    kept += 1;
                    fine += got.iter().filter(|v| **v % 257 != 0).count();
                }
            }
            assert!(fine > kept * 3 / 2, "{name}: {fine} of {}", kept * 3);
            // The box's red: on an HDR10 picture, PQ's paper-white red; on the others, as it is.
            let red = read.get_pixel(100, 175).0;
            let want = if name == "hdr10.avif" { [235.0, 51.0, 36.0].map(|c| (Curve::Pq.put(c / 255.0) * 65535.0).round() as u16) } else { [235 * 257, 51 * 257, 36 * 257] };
            assert_eq!(red, want, "{name}");
            // An HDR10 picture's PNG says what its numbers are: BT.2020, PQ (cICP).
            let bytes = std::fs::read(&copy).unwrap();
            let cicp = bytes.windows(8).position(|w| w == b"cICP\x09\x10\x00\x01");
            assert_eq!(cicp.is_some(), name == "hdr10.avif", "{name}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn marks_are_put_on_each_picture_s_curve() {
        // sRGB as it is; light with its curve off; PQ's paper white is 203 nits; HLG's 75 per cent.
        assert_eq!(Curve::Srgb.put(0.5), 0.5);
        assert!((Curve::Linear.put(1.0) - 1.0).abs() < 1e-6 && (Curve::Linear.put(0.5) - 0.214).abs() < 0.001);
        assert!((Curve::Pq.put(1.0) - 0.5807).abs() < 0.001, "{}", Curve::Pq.put(1.0));
        assert!((Curve::Hlg.put(1.0) - 0.75).abs() < 0.001, "{}", Curve::Hlg.put(1.0));
        // A PNG's cICP chunk: after IHDR, its CRC right (as the png crate reads it).
        let mut png = Vec::new();
        image::RgbImage::new(2, 2).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let tagged = with_cicp(png, (9, 16));
        let decoder = png::Decoder::new(std::io::Cursor::new(tagged));
        let reader = decoder.read_info().unwrap();
        let cicp = reader.info().coding_independent_code_points.unwrap();
        assert_eq!((cicp.color_primaries, cicp.transfer_function, cicp.matrix_coefficients, cicp.is_video_full_range_image), (9, 16, 0, true));
    }
}
