//! A JPEG's HDR gain map, kept through markup (the owner, 2026-11-19: "can it preserve too?").
//!
//! An Android phone's photograph (Ultra HDR), or a newer camera's or Apple's (ISO 21496-1),
//! carries after the picture a second, smaller JPEG: how much brighter each part of it is on a
//! screen that can show more than white. The first JPEG's MPF index (CIPA DC-007) says where it
//! is; what its values mean is in its own XMP (Adobe's gain map, `hdrgm`) or its ISO 21496-1 block
//! (APP2), or both. Saved, the gain map goes through the same turn and crop as the picture, at its
//! own scale; where marks are drawn it is set to no boost (a gain of 1), so the marks look as they
//! were drawn on an HDR screen rather than blown out; and it is written back after the new picture
//! with its own metadata as it was, the picture's XMP saying its new length and the MPF index made
//! true -- as libultrahdr writes one, which Android, Chrome and macOS read as HDR.
//!
//! A gain map of another kind (Apple's own before ISO 21496-1, whose meaning is in its maker
//! notes), or one whose metadata does not read, is not carried: the picture is saved as SDR, and
//! the question before it says so.

use image::{DynamicImage, ImageEncoder};

/// The namespaces and names by which a gain map is known.
const XMP: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";
const XMP_EXTENSION: &[u8] = b"http://ns.adobe.com/xmp/extension/\0";
const HDRGM: &str = "http://ns.adobe.com/hdr-gain-map/1.0/";
const ISO: &[u8] = b"urn:iso:std:iso:ts:21496:-1\0";
const MPF: &[u8] = b"MPF\0";

/// How a gain map is written again: high, as it is only a quarter the picture's size or less.
const QUALITY: u8 = 95;

/// What a JPEG holds of an HDR gain map.
#[derive(Debug, Clone, PartialEq)]
pub enum Found {
    /// None: an ordinary JPEG.
    None,
    /// One that goes with the picture saved.
    Carried(GainMap),
    /// One that does not, and why.
    Lost(String),
}

impl Found {
    /// For what the view asks: none, carried, or lost.
    pub fn kept(&self) -> Option<bool> {
        match self {
            Found::None => None,
            Found::Carried(_) => Some(true),
            Found::Lost(_) => Some(false),
        }
    }
}

/// A gain map as it was found.
#[derive(Debug, Clone, PartialEq)]
pub struct GainMap {
    /// The gain map's JPEG, whole.
    pub jpeg: Vec<u8>,
    /// Its own XMP and ISO 21496-1 segments, marker and all, carried as they are.
    pub own: Vec<Vec<u8>>,
    /// The picture's XMP (with its container's directory, its gain map's length in it), its
    /// extended XMP, and its ISO 21496-1 version block: carried, the length made true.
    pub primary_xmp: Option<Vec<u8>>,
    pub primary_extended: Vec<Vec<u8>>,
    pub primary_iso: Option<Vec<u8>>,
    /// The stored value, 0 to 1, that is a gain of 1 (no boost), for each channel.
    pub neutral: [f32; 3],
}

/// A JPEG's segments before its picture data: each marker, and where its body is.
fn segments(bytes: &[u8]) -> Vec<(u8, std::ops::Range<usize>)> {
    let mut out = Vec::new();
    if bytes.get(..2) != Some(&[0xFF, 0xD8]) {
        return out;
    }
    let mut at = 2usize;
    while at + 4 <= bytes.len() && bytes[at] == 0xFF {
        let marker = bytes[at + 1];
        if marker == 0xDA || marker == 0xD9 {
            break;
        }
        let length = u16::from_be_bytes([bytes[at + 2], bytes[at + 3]]) as usize;
        if length < 2 || at + 2 + length > bytes.len() {
            break;
        }
        out.push((marker, at + 4..at + 2 + length));
        at += 2 + length;
    }
    out
}

/// An image an MPF index names: its attribute, its size and its offset.
type MpEntry = (u32, usize, usize);

/// The images an MPF index names: each one's attribute, its size and its offset from the MPF's
/// TIFF header (`base`, in the file).
fn mp_entries(bytes: &[u8]) -> Option<(usize, Vec<MpEntry>)> {
    let (_, body) = segments(bytes).into_iter().find(|(m, r)| *m == 0xE2 && bytes[r.clone()].starts_with(MPF))?;
    let base = body.start + 4;
    let tiff = bytes.get(base..body.end)?;
    let little = match tiff.get(..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let u16_at = |i: usize| tiff.get(i..i + 2).map(|b| if little { u16::from_le_bytes([b[0], b[1]]) } else { u16::from_be_bytes([b[0], b[1]]) });
    let u32_at = |i: usize| tiff.get(i..i + 4).map(|b| if little { u32::from_le_bytes([b[0], b[1], b[2], b[3]]) } else { u32::from_be_bytes([b[0], b[1], b[2], b[3]]) });
    let ifd = u32_at(4)? as usize;
    for k in 0..u16_at(ifd)? as usize {
        let e = ifd + 2 + k * 12;
        if u16_at(e)? == 0xB002 {
            let (count, list) = (u32_at(e + 4)? as usize / 16, u32_at(e + 8)? as usize);
            let entries = (0..count).map(|i| Some((u32_at(list + i * 16)?, u32_at(list + i * 16 + 4)? as usize, u32_at(list + i * 16 + 8)? as usize))).collect::<Option<Vec<_>>>()?;
            return Some((base, entries));
        }
    }
    None
}

/// The values of an XMP property of the namespace `ns`, named `name`: an attribute, an element,
/// or an element holding a sequence (one per channel).
fn xmp_values(xmp: &str, ns: &str, name: &str) -> Option<Vec<f32>> {
    // The prefix the namespace is bound to in this packet.
    let bound = xmp.find(&format!("=\"{ns}\"")).or_else(|| xmp.find(&format!("='{ns}'")))?;
    let before = &xmp[..bound];
    let prefix = &before[before.rfind("xmlns:")? + 6..];
    let qualified = format!("{prefix}:{name}");
    let number = |s: &str| s.trim().parse::<f32>().ok();
    // An attribute: prefix:Name="v".
    for quote in ['"', '\''] {
        let attribute = format!("{qualified}={quote}");
        if let Some(at) = xmp.find(&attribute) {
            let rest = &xmp[at + attribute.len()..];
            return number(&rest[..rest.find(quote)?]).map(|v| vec![v]);
        }
    }
    // An element: <prefix:Name>v</prefix:Name>, or a sequence of rdf:li inside it.
    let open = format!("<{qualified}>");
    let at = xmp.find(&open)? + open.len();
    let end = at + xmp[at..].find(&format!("</{qualified}>"))?;
    let inner = &xmp[at..end];
    if !inner.contains("<") {
        return number(inner).map(|v| vec![v]);
    }
    let values: Vec<f32> = inner.split("<rdf:li").skip(1).filter_map(|li| number(&li[li.find('>')? + 1..li.find("</rdf:li")?])).collect();
    (!values.is_empty()).then_some(values)
}

/// For each channel, a property's values, the one value for all if there is one.
fn per_channel(values: Option<Vec<f32>>, default: f32) -> [f32; 3] {
    match values.as_deref() {
        Some([v]) => [*v; 3],
        Some([r, g, b, ..]) => [*r, *g, *b],
        _ => [default; 3],
    }
}

/// The value that is no boost, given the gain map's least and most (log2) and its gamma: as
/// libultrahdr encodes one, `((log2 gain - min) / (max - min)) ^ gamma`.
fn neutral(min: f32, max: f32, gamma: f32) -> f32 {
    let span = max - min;
    let mapped = if span.abs() < 1e-6 { 0.0 } else { ((0.0 - min) / span).clamp(0.0, 1.0) };
    if gamma > 0.0 && (gamma - 1.0).abs() > 1e-6 { mapped.powf(gamma) } else { mapped }
}

/// What a gain map's own XMP says: no boost, per channel; `None` without `GainMapMax`.
fn xmp_neutral(xmp: &str) -> Option<[f32; 3]> {
    let max = per_channel(Some(xmp_values(xmp, HDRGM, "GainMapMax")?), 0.0);
    let min = per_channel(xmp_values(xmp, HDRGM, "GainMapMin"), 0.0);
    let gamma = per_channel(xmp_values(xmp, HDRGM, "Gamma"), 1.0);
    Some([0, 1, 2].map(|c| neutral(min[c], max[c], gamma[c])))
}

/// What a gain map's ISO 21496-1 block says (after its name): no boost, per channel. As
/// libultrahdr reads it: its least version, its writer's, its flags (several channels, a common
/// denominator), the headrooms, then for each channel its least and most gain, gamma and offsets,
/// each a fraction.
fn iso_neutral(block: &[u8]) -> Option<[f32; 3]> {
    let mut at = 0usize;
    let mut take = |n: usize| -> Option<&[u8]> {
        let piece = block.get(at..at + n)?;
        at += n;
        Some(piece)
    };
    let u16 = |b: &[u8]| u16::from_be_bytes([b[0], b[1]]);
    let u32 = |b: &[u8]| u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
    let least = u16(take(2)?);
    let _writer = u16(take(2)?);
    if least != 0 {
        return None;
    }
    let flags = take(1)?[0];
    let channels = if flags & 0x80 != 0 { 3 } else { 1 };
    let common = flags & 0x08 != 0;
    let fraction = |n: i64, d: u32| if d == 0 { None } else { Some(n as f32 / d as f32) };
    let mut gains = Vec::new();
    if common {
        let d = u32(take(4)?);
        take(8)?; // the base's and the alternate's headroom
        for _ in 0..channels {
            let min = fraction(u32(take(4)?) as i32 as i64, d)?;
            let max = fraction(u32(take(4)?) as i32 as i64, d)?;
            let gamma = fraction(u32(take(4)?) as i64, d)?;
            take(8)?; // the offsets
            gains.push((min, max, gamma));
        }
    } else {
        take(16)?;
        for _ in 0..channels {
            let min = fraction(u32(take(4)?) as i32 as i64, u32(take(4)?))?;
            let max = fraction(u32(take(4)?) as i32 as i64, u32(take(4)?))?;
            let gamma = fraction(u32(take(4)?) as i64, u32(take(4)?))?;
            take(16)?;
            gains.push((min, max, gamma));
        }
    }
    let one = |c: usize| {
        let (min, max, gamma) = gains[c.min(gains.len() - 1)];
        neutral(min, max, gamma)
    };
    Some([one(0), one(1), one(2)])
}

/// What `bytes`, a JPEG, holds of a gain map.
pub fn find(bytes: &[u8]) -> Found {
    let primary = segments(bytes);
    let body = |r: &std::ops::Range<usize>| &bytes[r.clone()];
    let primary_xmp = primary.iter().find(|(m, r)| *m == 0xE1 && body(r).starts_with(XMP)).map(|(_, r)| body(r).to_vec());
    let primary_iso = primary.iter().find(|(m, r)| *m == 0xE2 && body(r).starts_with(ISO)).map(|(_, r)| body(r).to_vec());
    let says_hdr = primary_xmp.as_ref().is_some_and(|x| {
        let text = String::from_utf8_lossy(x);
        text.contains(HDRGM) || text.contains("Semantic=\"GainMap\"")
    }) || primary_iso.is_some();
    let Some((base, entries)) = mp_entries(bytes) else {
        return if says_hdr { Found::Lost("its HDR layer is not where SpatiLoupe can find it (no MPF index)".into()) } else { Found::None };
    };
    for &(_, size, offset) in entries.iter().skip(1) {
        let Some(jpeg) = bytes.get(base + offset..base + offset + size).filter(|j| j.starts_with(&[0xFF, 0xD8])) else { continue };
        let own = segments(jpeg);
        let xmp = own.iter().find(|(m, r)| *m == 0xE1 && jpeg[r.clone()].starts_with(XMP)).map(|(_, r)| String::from_utf8_lossy(&jpeg[r.clone()]).into_owned());
        let iso = own.iter().find(|(m, r)| *m == 0xE2 && jpeg[r.clone()].starts_with(ISO)).map(|(_, r)| &jpeg[r.start + ISO.len()..r.end]);
        // Apple's own gain map, before ISO 21496-1: its meaning is in the maker notes.
        if xmp.as_deref().is_some_and(|x| x.contains("hdrgainmap")) && iso.is_none() && !xmp.as_deref().is_some_and(|x| x.contains(HDRGM)) {
            return Found::Lost("its HDR layer is Apple's own kind, which SpatiLoupe cannot carry over".into());
        }
        let neutral = xmp.as_deref().filter(|x| x.contains(HDRGM)).and_then(xmp_neutral).or_else(|| iso.and_then(iso_neutral));
        let gain_map = xmp.as_deref().is_some_and(|x| x.contains(HDRGM)) || iso.is_some();
        if !gain_map {
            // A depth map, a preview, another eye: not a gain map.
            continue;
        }
        let Some(neutral) = neutral else {
            return Found::Lost("its HDR layer's description does not read".into());
        };
        if image::load_from_memory_with_format(jpeg, image::ImageFormat::Jpeg).is_err() {
            return Found::Lost("its HDR layer's picture does not read".into());
        }
        let own_segments = own
            .iter()
            .filter(|(m, r)| (*m == 0xE1 && jpeg[r.clone()].starts_with(XMP)) || (*m == 0xE2 && jpeg[r.clone()].starts_with(ISO)))
            .map(|(m, r)| segment(*m, &jpeg[r.clone()]))
            .collect();
        let primary_extended = primary.iter().filter(|(m, r)| *m == 0xE1 && body(r).starts_with(XMP_EXTENSION)).map(|(_, r)| body(r).to_vec()).collect();
        // The picture's XMP must say the gain map's new length if it says one now.
        if let Some(x) = &primary_xmp {
            if length_in(&String::from_utf8_lossy(x)).is_none() && String::from_utf8_lossy(x).contains("Semantic=\"GainMap\"") {
                return Found::Lost("its HDR layer's length is written where SpatiLoupe cannot change it".into());
            }
        }
        return Found::Carried(GainMap { jpeg: jpeg.to_vec(), own: own_segments, primary_xmp, primary_extended, primary_iso, neutral });
    }
    if says_hdr {
        return Found::Lost("its HDR layer is not where SpatiLoupe can find it".into());
    }
    Found::None
}

/// A segment written whole: its marker, its length and its body.
fn segment(marker: u8, body: &[u8]) -> Vec<u8> {
    let mut out = vec![0xFF, marker];
    out.extend_from_slice(&((body.len() + 2) as u16).to_be_bytes());
    out.extend_from_slice(body);
    out
}

/// Where in an XMP packet the gain map's length is: the digits of `Length="..."` in the
/// container's item whose semantic is the gain map.
fn length_in(xmp: &str) -> Option<std::ops::Range<usize>> {
    let item = xmp.find("Semantic=\"GainMap\"")?;
    // The item's element: from its `<` to its end.
    let start = xmp[..item].rfind('<')?;
    let end = item + xmp[item..].find('>')?;
    let element = &xmp[start..end];
    let at = element.find("Length=\"")? + "Length=\"".len();
    let digits = element[at..].find('"')?;
    Some(start + at..start + at + digits)
}

/// The picture's XMP made true of what is written: the gain map's length, and the orientation
/// upright (the pixels are written so).
fn primary_xmp(xmp: &[u8], length: usize) -> Vec<u8> {
    let mut text = String::from_utf8_lossy(xmp).into_owned();
    if let Some(range) = length_in(&text) {
        text.replace_range(range, &length.to_string());
    }
    for (from, to) in [("tiff:Orientation=\"", "\""), ("<tiff:Orientation>", "<")] {
        if let Some(at) = text.find(from) {
            let at = at + from.len();
            if let Some(end) = text[at..].find(to) {
                text.replace_range(at..at + end, "1");
            }
        }
    }
    text.into_bytes()
}

/// The JPEG `written` with the XMP of the JPEG it was made from (`original`) -- and its extended
/// XMP -- carried over, its orientation made upright: a JPEG without a gain map saved keeps its
/// rating, its keywords and its description, which the image crate does not keep.
pub fn with_xmp(written: &[u8], original: &[u8]) -> Vec<u8> {
    let carried: Vec<Vec<u8>> = segments(original)
        .into_iter()
        .filter(|(m, r)| *m == 0xE1 && (original[r.clone()].starts_with(XMP) || original[r.clone()].starts_with(XMP_EXTENSION)))
        .map(|(m, r)| {
            let body = &original[r];
            if body.starts_with(XMP) { segment(m, &primary_xmp(body, 0)) } else { segment(m, body) }
        })
        .collect();
    if carried.is_empty() {
        return written.to_vec();
    }
    with_segments(written, &carried)
}

/// `jpeg` with `added` segments after the segments it begins with (JFIF's, Exif's, its profile's).
fn with_segments(jpeg: &[u8], added: &[Vec<u8>]) -> Vec<u8> {
    let at = segments(jpeg).iter().take_while(|(m, _)| (0xE0..=0xEF).contains(m)).last().map_or(2, |(_, r)| r.end);
    let mut out = jpeg[..at].to_vec();
    for s in added {
        out.extend_from_slice(s);
    }
    out.extend_from_slice(&jpeg[at..]);
    out
}

/// An MPF segment for two images: the picture, `primary` bytes long from the file's start, and
/// the gain map, `gain` bytes long at `offset` from the segment's TIFF header -- big-endian, as
/// libultrahdr writes one.
fn mpf(primary: usize, gain: usize, offset: usize) -> Vec<u8> {
    let mut tiff = b"MM\0*".to_vec();
    tiff.extend_from_slice(&8u32.to_be_bytes());
    // The index: three entries, then the next IFD (none).
    tiff.extend_from_slice(&3u16.to_be_bytes());
    let entry = |tiff: &mut Vec<u8>, tag: u16, kind: u16, count: u32, value: [u8; 4]| {
        tiff.extend_from_slice(&tag.to_be_bytes());
        tiff.extend_from_slice(&kind.to_be_bytes());
        tiff.extend_from_slice(&count.to_be_bytes());
        tiff.extend_from_slice(&value);
    };
    entry(&mut tiff, 0xB000, 7, 4, *b"0100");
    entry(&mut tiff, 0xB001, 4, 1, 2u32.to_be_bytes());
    // The entries themselves after the index: 8 + 2 + 3 * 12 + 4 = 50.
    entry(&mut tiff, 0xB002, 7, 32, 50u32.to_be_bytes());
    tiff.extend_from_slice(&0u32.to_be_bytes());
    // The picture: a baseline primary image (0x030000), at the file's start.
    for (attribute, size, at) in [(0x0003_0000u32, primary, 0usize), (0, gain, offset)] {
        tiff.extend_from_slice(&attribute.to_be_bytes());
        tiff.extend_from_slice(&(size as u32).to_be_bytes());
        tiff.extend_from_slice(&(at as u32).to_be_bytes());
        tiff.extend_from_slice(&[0; 4]);
    }
    let mut body = MPF.to_vec();
    body.extend_from_slice(&tiff);
    segment(0xE2, &body)
}

/// The gain map made as the picture was: put upright as the picture's camera said (`orientation`),
/// turned `turns` quarters, set to no boost under the marks' `layer` (at the turned picture's
/// size, `size`), and cropped to `crop` (in the turned picture's pixels), at its own scale.
pub fn remade(gain: &GainMap, orientation: Option<image::metadata::Orientation>, turns: u8, layer: &[u8], size: (u32, u32), crop: Option<(u32, u32, u32, u32)>) -> Result<DynamicImage, String> {
    let mut map = image::load_from_memory_with_format(&gain.jpeg, image::ImageFormat::Jpeg).map_err(|e| e.to_string())?;
    if let Some(orientation) = orientation {
        map.apply_orientation(orientation);
    }
    let mut map = crate::depth::turned(map, turns);
    let (gw, gh) = (map.width(), map.height());
    let (sx, sy) = (gw as f32 / size.0.max(1) as f32, gh as f32 / size.1.max(1) as f32);
    // Under the marks: how much of each of the gain map's pixels they cover, the most of any
    // picture pixel in it -- then a pixel further all round, which a reader's smoothing reaches.
    if crate::depth::any(layer) {
        let mut cover = vec![0u8; (gw * gh) as usize];
        for (i, mark) in layer.chunks_exact(4).enumerate() {
            if mark[3] != 0 {
                let (x, y) = ((i as u32 % size.0) as f32 * sx, (i as u32 / size.0) as f32 * sy);
                let at = ((y as u32).min(gh - 1) * gw + (x as u32).min(gw - 1)) as usize;
                cover[at] = cover[at].max(mark[3]);
            }
        }
        let mut grown = cover.clone();
        for y in 0..gh as i64 {
            for x in 0..gw as i64 {
                let mut most = 0u8;
                for (dx, dy) in [(-1i64, -1i64), (0, -1), (1, -1), (-1, 0), (0, 0), (1, 0), (-1, 1), (0, 1), (1, 1)] {
                    let (nx, ny) = (x + dx, y + dy);
                    if nx >= 0 && ny >= 0 && nx < gw as i64 && ny < gh as i64 {
                        most = most.max(cover[(ny * gw as i64 + nx) as usize]);
                    }
                }
                grown[(y * gw as i64 + x) as usize] = most;
            }
        }
        let neutral = gain.neutral.map(|v| v * 255.0);
        let blend = |v: u8, n: f32, c: u8| (v as f32 + (n - v as f32) * c as f32 / 255.0).round().clamp(0.0, 255.0) as u8;
        match &mut map {
            DynamicImage::ImageLuma8(b) => {
                for (p, c) in b.pixels_mut().zip(&grown) {
                    p.0[0] = blend(p.0[0], neutral[0], *c);
                }
            }
            other => {
                let mut rgb = other.to_rgb8();
                for (p, c) in rgb.pixels_mut().zip(&grown) {
                    for k in 0..3 {
                        p.0[k] = blend(p.0[k], neutral[k], *c);
                    }
                }
                *other = DynamicImage::ImageRgb8(rgb);
            }
        }
    }
    if let Some((x, y, w, h)) = crop {
        let x0 = ((x as f32 * sx).round() as u32).min(gw - 1);
        let y0 = ((y as f32 * sy).round() as u32).min(gh - 1);
        let x1 = (((x + w) as f32 * sx).round() as u32).clamp(x0 + 1, gw);
        let y1 = (((y + h) as f32 * sy).round() as u32).clamp(y0 + 1, gh);
        map = map.crop_imm(x0, y0, x1 - x0, y1 - y0);
    }
    Ok(map)
}

/// The whole JPEG to be written: the new picture (`primary`, as the image crate wrote it, with its
/// Exif and profile), its XMP and ISO block carried and an MPF index, then the gain map `map`
/// with its own metadata as it was.
pub fn written(primary: &[u8], gain: &GainMap, map: &DynamicImage) -> Result<Vec<u8>, String> {
    let mut encoded = Vec::new();
    let (data, kind) = match map {
        DynamicImage::ImageLuma8(b) => (b.as_raw().clone(), image::ExtendedColorType::L8),
        other => (other.to_rgb8().into_raw(), image::ExtendedColorType::Rgb8),
    };
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut encoded, QUALITY).write_image(&data, map.width(), map.height(), kind).map_err(|e| e.to_string())?;
    let gain_jpeg = with_segments(&encoded, &gain.own);
    let mut added = Vec::new();
    if let Some(xmp) = &gain.primary_xmp {
        added.push(segment(0xE1, &primary_xmp(xmp, gain_jpeg.len())));
    }
    for extended in &gain.primary_extended {
        added.push(segment(0xE1, extended));
    }
    if let Some(iso) = &gain.primary_iso {
        added.push(segment(0xE2, iso));
    }
    // The index's own place: after the rest, its size known before its values are.
    let without = with_segments(primary, &added);
    let at = segments(&without).iter().take_while(|(m, _)| (0xE0..=0xEF).contains(m)).last().map_or(2, |(_, r)| r.end);
    let length = without.len() + mpf(0, 0, 0).len();
    // Offsets count from the MPF's TIFF header: its segment's start, marker, length and "MPF\0".
    let base = at + 4 + MPF.len();
    let index = mpf(length, gain_jpeg.len(), length - base);
    let mut out = without[..at].to_vec();
    out.extend_from_slice(&index);
    out.extend_from_slice(&without[at..]);
    out.extend_from_slice(&gain_jpeg);
    Ok(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// An Ultra HDR JPEG as libultrahdr lays one out, made here from its parts: a picture (`w` by
    /// `h`, grey with a light left half) with Exif (orientation `turn`), its XMP naming the gain
    /// map in its container, ISO 21496-1's version block and an MPF index; then the gain map, a
    /// quarter its size, its value the column it is in (so a turn or crop shows where it went),
    /// with its XMP (`hdrgm`: least 0, most 2, gamma 1) and its ISO 21496-1 metadata.
    pub fn ultra_hdr(w: u32, h: u32, exif: Option<Vec<u8>>) -> Vec<u8> {
        made(w, h, exif, false)
    }

    /// The same, or (`apple`) with Apple's own gain map before ISO 21496-1: its XMP naming it an
    /// auxiliary image, its meaning in the maker notes, no `hdrgm` and no ISO block.
    pub fn made(w: u32, h: u32, exif: Option<Vec<u8>>, apple: bool) -> Vec<u8> {
        let picture = image::RgbImage::from_fn(w, h, |x, _| if x < w / 2 { image::Rgb([200, 200, 200]) } else { image::Rgb([90, 90, 90]) });
        let (gw, gh) = (w / 4, h / 4);
        let map = image::GrayImage::from_fn(gw, gh, |x, _| image::Luma([(40 + x * 200 / gw.max(1)) as u8]));
        let mut gain = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut gain, 100).write_image(map.as_raw(), gw, gh, image::ExtendedColorType::L8).unwrap();
        let gain_xmp = format!(
            "{}<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"><rdf:Description xmlns:hdrgm=\"{HDRGM}\" hdrgm:Version=\"1.0\" hdrgm:GainMapMin=\"0\" hdrgm:GainMapMax=\"2\" hdrgm:Gamma=\"1\" hdrgm:OffsetSDR=\"0.015625\" hdrgm:OffsetHDR=\"0.015625\" hdrgm:HDRCapacityMin=\"0\" hdrgm:HDRCapacityMax=\"2\" hdrgm:BaseRenditionIsHDR=\"False\"/></rdf:RDF></x:xmpmeta>",
            String::from_utf8_lossy(XMP)
        );
        // ISO 21496-1: versions 0, flags 0 (one channel, own denominators), headrooms 0 and 2,
        // then min 0/1, max 2/1, gamma 1/1, offsets 1/64 each.
        let mut iso = ISO.to_vec();
        iso.extend_from_slice(&[0, 0, 0, 0, 0]);
        for (n, d) in [(0i32, 1u32), (2, 1), (0, 1), (2, 1), (1, 1), (1, 64), (1, 64)] {
            iso.extend_from_slice(&n.to_be_bytes());
            iso.extend_from_slice(&d.to_be_bytes());
        }
        let apple_xmp = format!("{}<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"><rdf:Description xmlns:apdi=\"http://ns.apple.com/pixeldatainfo/1.0/\" apdi:AuxiliaryImageType=\"urn:com:apple:photo:2020:aux:hdrgainmap\"/></rdf:RDF></x:xmpmeta>", String::from_utf8_lossy(XMP));
        let gain = if apple { with_segments(&gain, &[segment(0xE1, apple_xmp.as_bytes())]) } else { with_segments(&gain, &[segment(0xE1, gain_xmp.as_bytes()), segment(0xE2, &iso)]) };
        let mut primary = Vec::new();
        let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut primary, 95);
        if let Some(exif) = exif {
            encoder.set_exif_metadata(exif).unwrap();
        }
        encoder.write_image(picture.as_raw(), w, h, image::ExtendedColorType::Rgb8).unwrap();
        let xmp = |length: usize| {
            format!(
                "{}<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"><rdf:Description xmlns:Container=\"http://ns.google.com/photos/1.0/container/\" xmlns:Item=\"http://ns.google.com/photos/1.0/container/item/\" xmlns:hdrgm=\"{HDRGM}\" hdrgm:Version=\"1.0\"><Container:Directory><rdf:Seq><rdf:li rdf:parseType=\"Resource\"><Container:Item Item:Semantic=\"Primary\" Item:Mime=\"image/jpeg\"/></rdf:li><rdf:li rdf:parseType=\"Resource\"><Container:Item Item:Semantic=\"GainMap\" Item:Mime=\"image/jpeg\" Item:Length=\"{length}\"/></rdf:li></rdf:Seq></Container:Directory></rdf:Description></rdf:RDF></x:xmpmeta>",
                String::from_utf8_lossy(XMP)
            )
        };
        let mut version = ISO.to_vec();
        version.extend_from_slice(&[0, 0, 0, 0]);
        let head = if apple { primary.clone() } else { with_segments(&primary, &[segment(0xE1, xmp(gain.len()).as_bytes()), segment(0xE2, &version)]) };
        // The MPF after them, by hand (as the spec has it, not as `mpf` above writes it):
        // little-endian, to read both.
        let at = segments(&head).iter().take_while(|(m, _)| (0xE0..=0xEF).contains(m)).last().unwrap().1.end;
        let mut tiff = b"II*\0".to_vec();
        tiff.extend_from_slice(&8u32.to_le_bytes());
        tiff.extend_from_slice(&3u16.to_le_bytes());
        for (tag, kind, count, value) in [(0xB000u16, 7u16, 4u32, *b"0100"), (0xB001, 4, 1, 2u32.to_le_bytes()), (0xB002, 7, 32, 50u32.to_le_bytes())] {
            tiff.extend_from_slice(&tag.to_le_bytes());
            tiff.extend_from_slice(&kind.to_le_bytes());
            tiff.extend_from_slice(&count.to_le_bytes());
            tiff.extend_from_slice(&value);
        }
        tiff.extend_from_slice(&0u32.to_le_bytes());
        let index_length = 4 + 4 + tiff.len() + 32;
        let total = head.len() + index_length;
        let base = at + 8;
        for (attribute, size, offset) in [(0x0003_0000u32, total, 0usize), (0, gain.len(), total - base)] {
            tiff.extend_from_slice(&attribute.to_le_bytes());
            tiff.extend_from_slice(&(size as u32).to_le_bytes());
            tiff.extend_from_slice(&(offset as u32).to_le_bytes());
            tiff.extend_from_slice(&[0; 4]);
        }
        let mut body = MPF.to_vec();
        body.extend_from_slice(&tiff);
        let mut out = head[..at].to_vec();
        out.extend_from_slice(&segment(0xE2, &body));
        out.extend_from_slice(&head[at..]);
        assert_eq!(out.len(), total);
        out.extend_from_slice(&gain);
        out
    }

    #[test]
    fn an_ultra_hdr_jpeg_s_gain_map_is_found_and_its_no_boost_read() {
        let file = ultra_hdr(64, 48, None);
        let Found::Carried(gain) = find(&file) else { panic!("{:?}", find(&file)) };
        // Least 0, most 2 (log2), gamma 1: no boost is 0.
        assert_eq!(gain.neutral, [0.0; 3]);
        assert_eq!(gain.own.len(), 2, "its XMP and its ISO block");
        assert!(gain.primary_xmp.is_some() && gain.primary_iso.is_some());
        let map = image::load_from_memory(&gain.jpeg).unwrap();
        assert_eq!((map.width(), map.height()), (16, 12));
        // Other least and most, per channel, as elements; ISO's own.
        let xmp = format!("<x:xmpmeta><rdf:Description xmlns:g=\"{HDRGM}\"><g:GainMapMin><rdf:Seq><rdf:li>-1</rdf:li><rdf:li>-1</rdf:li><rdf:li>0.5</rdf:li></rdf:Seq></g:GainMapMin><g:GainMapMax>3</g:GainMapMax><g:Gamma>2</g:Gamma></rdf:Description></x:xmpmeta>");
        let n = xmp_neutral(&xmp).unwrap();
        assert!((n[0] - 0.0625).abs() < 1e-6 && (n[2] - 0.0).abs() < 1e-6, "{n:?}");
        let mut iso = vec![0, 0, 0, 0, 0x08];
        for v in [4u32, 0, 8, (-4i32) as u32, 4, 4] {
            iso.extend_from_slice(&v.to_be_bytes());
        }
        iso.extend_from_slice(&[0; 8]);
        assert_eq!(iso_neutral(&iso), Some([0.5; 3]), "min -1, max 1, gamma 1 over a common 4");
        // A plain JPEG has none; one saying HDR but with no index loses it.
        let mut plain = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut plain).write_image(&[0u8; 12], 2, 2, image::ExtendedColorType::Rgb8).unwrap();
        assert_eq!(find(&plain), Found::None);
        let mpf_at = segments(&file).into_iter().find(|(m, r)| *m == 0xE2 && file[r.clone()].starts_with(MPF)).unwrap().1;
        let mut broken = file.clone();
        broken[mpf_at.start..mpf_at.start + 4].copy_from_slice(b"XYZ\0");
        assert!(matches!(find(&broken), Found::Lost(_)));
    }

    /// A camera's account saying only to turn the picture a quarter clockwise (6).
    fn sideways() -> Vec<u8> {
        let mut t = b"II*\0".to_vec();
        t.extend_from_slice(&8u32.to_le_bytes());
        t.extend_from_slice(&1u16.to_le_bytes());
        t.extend_from_slice(&0x0112u16.to_le_bytes());
        t.extend_from_slice(&3u16.to_le_bytes());
        t.extend_from_slice(&1u32.to_le_bytes());
        t.extend_from_slice(&6u16.to_le_bytes());
        t.extend_from_slice(&[0, 0]);
        t.extend_from_slice(&0u32.to_le_bytes());
        t
    }

    #[test]
    fn a_gain_map_is_turned_cropped_and_made_neutral_under_the_marks_with_the_picture() {
        use crate::markup::{Area, Ink, Marks, Note, Shape, Weight};
        let dir = std::env::temp_dir().join(format!("spatiloupe-gainmap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("PXL_1.jpg");
        // Stored 64 by 48, its camera saying to turn it a quarter clockwise (shown 48 by 64); its
        // gain map 16 by 12, each column its own value.
        std::fs::write(&path, ultra_hdr(64, 48, Some(sideways()))).unwrap();
        // Turned a quarter more while marked (64 by 48 again, upside down from how it is stored),
        // a box drawn, cropped to (8, 8)-(48, 40).
        let marks = Marks {
            notes: vec![Note { page: 0, shape: Shape::Box(Area::new(30.0, 20.0, 50.0, 40.0)), ink: Ink::Red, weight: Weight::Medium, size: 4.0 }],
            crops: vec![(0, Area::new(8.0, 8.0, 48.0, 40.0))],
            turns: 1,
        };
        let saved = crate::save::save(&crate::save::Saving { source: path.clone(), marks, frame: (64.0, 48.0), copy: false }).unwrap();
        let bytes = std::fs::read(&saved).unwrap();
        // The picture, as shown and cropped, upright.
        let picture = image::load_from_memory(&bytes).unwrap();
        assert_eq!((picture.width(), picture.height()), (40, 32));
        let exif = exif::Reader::new().read_from_container(&mut std::io::Cursor::new(&bytes)).unwrap();
        assert_eq!(exif.get_field(exif::Tag::Orientation, exif::In::PRIMARY).and_then(|f| f.value.get_uint(0)), Some(1));
        // An MPF of two images, the second the gain map, where the index says.
        let (base, entries) = mp_entries(&bytes).expect("an MPF index");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0], (0x0003_0000, base + entries[1].2, 0), "the picture: all before the gain map");
        let (_, size, offset) = entries[1];
        assert_eq!(base + offset + size, bytes.len());
        let map_jpeg = &bytes[base + offset..];
        let map = image::load_from_memory(map_jpeg).unwrap().to_luma8();
        // Cropped as the picture was at its own scale, a quarter: (2, 2)-(12, 10).
        assert_eq!(map.dimensions(), (10, 8));
        // Turned twice, a column's value is now its mirror's: at (x, y), stored column 13 - x.
        // Clear of the box (its line, and a pixel round it): before it, above it, and inside it.
        for (x, y) in [(0u32, 0u32), (1, 6), (8, 0), (7, 5)] {
            let want = 40.0 + (13 - x) as f32 * 200.0 / 16.0;
            let got = map.get_pixel(x, y).0[0] as f32;
            assert!((got - want).abs() <= 6.0, "({x}, {y}) {got} {want}");
        }
        // Under the box's line (x 30 of the picture is the map's 7.5, cropped 5.5): no boost, 0.
        for y in 3..8 {
            let got = map.get_pixel(5, y).0[0];
            assert!(got <= 6, "(5, {y}) {got}");
        }
        // Its metadata as it was, and the picture's XMP saying the gain map's new length.
        let Found::Carried(again) = find(&bytes) else { panic!("{:?}", find(&bytes)) };
        assert_eq!(again.neutral, [0.0; 3]);
        assert_eq!(again.own.len(), 2);
        let xmp = String::from_utf8_lossy(again.primary_xmp.as_ref().unwrap()).into_owned();
        assert!(xmp.contains(&format!("Item:Length=\"{size}\"")), "{xmp}");
        assert!(again.primary_iso.is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_gain_map_that_cannot_be_carried_is_said_and_the_picture_saved_as_sdr() {
        use crate::markup::{Area, Ink, Marks, Note, Shape, Weight};
        let file = made(64, 48, None, true);
        let Found::Lost(why) = find(&file) else { panic!("{:?}", find(&file)) };
        assert_eq!(why, "its HDR layer is Apple's own kind, which SpatiLoupe cannot carry over");
        assert_eq!(find(&file).kept(), Some(false));
        let dir = std::env::temp_dir().join(format!("spatiloupe-gainmap-lost-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("IMG_1.jpg");
        std::fs::write(&path, &file).unwrap();
        // As the view sees it when it is read.
        assert_eq!(crate::picture::read(&path, 256).unwrap().saves.hdr, Some(false));
        let marks = Marks { notes: vec![Note { page: 0, shape: Shape::Box(Area::new(10.0, 10.0, 30.0, 30.0)), ink: Ink::Red, weight: Weight::Medium, size: 4.0 }], ..Marks::default() };
        let copy = crate::save::save(&crate::save::Saving { source: path.clone(), marks, frame: (64.0, 48.0), copy: true }).unwrap();
        let bytes = std::fs::read(&copy).unwrap();
        // The picture alone: SDR, no index, nothing after it.
        assert!(mp_entries(&bytes).is_none());
        assert_eq!(find(&bytes), Found::None);
        assert_eq!(image::load_from_memory(&bytes).unwrap().width(), 64);
        // An Ultra HDR one, as read: carried.
        std::fs::write(&path, ultra_hdr(64, 48, None)).unwrap();
        assert_eq!(crate::picture::read(&path, 256).unwrap().saves.hdr, Some(true));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An Ultra HDR JPEG made as the tests make one, and the same marked up, cropped and turned by
    /// SpatiLoupe, written into a folder for other readers to be shown:
    /// `SPATILOUPE_OUT=/some/folder cargo test -p spatiloupe ultra_hdr_for_readers -- --ignored`.
    #[test]
    #[ignore]
    fn ultra_hdr_for_readers() {
        use crate::markup::{Area, Ink, Marks, Note, Shape, Weight};
        let dir = std::path::PathBuf::from(std::env::var("SPATILOUPE_OUT").expect("SPATILOUPE_OUT"));
        std::fs::write(dir.join("ultra-hdr.jpg"), ultra_hdr(640, 480, None)).unwrap();
        // And one with Apple's own kind of gain map, which is not carried.
        std::fs::write(dir.join("apple-hdr.jpg"), made(640, 480, None, true)).unwrap();
        std::fs::copy(dir.join("ultra-hdr.jpg"), dir.join("ultra-hdr-marked.jpg")).unwrap();
        let marks = Marks {
            notes: vec![Note { page: 0, shape: Shape::Box(Area::new(200.0, 100.0, 500.0, 300.0)), ink: Ink::Red, weight: Weight::Medium, size: 8.0 }],
            crops: vec![(0, Area::new(40.0, 40.0, 600.0, 440.0))],
            turns: 1,
        };
        crate::save::save(&crate::save::Saving { source: dir.join("ultra-hdr-marked.jpg"), marks, frame: (480.0, 640.0), copy: false }).unwrap();
    }
}
