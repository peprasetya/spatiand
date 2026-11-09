//! Whether a picture is two eyes' worth, and how they are packed.
//!
//! Stereo cameras write the two eyes three ways: as two images in one file (the MPO a 3D
//! camera makes), or side by side or one above the other in one picture -- which is how the
//! Beam Pro's own camera writes them, and how most stereo photographs found anywhere else are
//! shared. A name sometimes says (`_sbs`, `.jps`); more often nothing does, and then the
//! picture itself is asked: the two halves of a stereo pair are the same scene seen from a
//! few centimetres apart, which the two halves of any other picture almost never are.

use std::path::Path;

/// How a picture's two eyes are packed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Eyes {
    /// One picture for both eyes.
    Mono,
    /// Left eye's view on the left.
    SideBySide,
    /// Right eye's view on the left, as cross-eyed viewing wants and `.jps` files are.
    Crossed,
    /// Left eye's view on top.
    TopBottom,
    /// Two pictures: [`crate::picture::Picture::right`] is the right eye.
    Pair,
}

impl Eyes {
    pub fn is_stereo(self) -> bool {
        self != Eyes::Mono
    }

    pub fn name(self) -> &'static str {
        match self {
            Eyes::Mono => "2D",
            Eyes::SideBySide => "Side by side",
            Eyes::Crossed => "Side by side, right eye first",
            Eyes::TopBottom => "Over and under",
            Eyes::Pair => "Two pictures",
        }
    }

    /// Where each eye is in the picture, as (left eye, right eye) rectangles of texture
    /// coordinates: left, top, right, bottom.
    pub fn halves(self) -> ([f32; 4], [f32; 4]) {
        match self {
            Eyes::Mono | Eyes::Pair => ([0.0, 0.0, 1.0, 1.0], [0.0, 0.0, 1.0, 1.0]),
            Eyes::SideBySide => ([0.0, 0.0, 0.5, 1.0], [0.5, 0.0, 1.0, 1.0]),
            Eyes::Crossed => ([0.5, 0.0, 1.0, 1.0], [0.0, 0.0, 0.5, 1.0]),
            Eyes::TopBottom => ([0.0, 0.0, 1.0, 0.5], [0.0, 0.5, 1.0, 1.0]),
        }
    }
}

/// What a name says, if it says anything.
fn by_name(path: &Path) -> Option<Eyes> {
    let name = path.file_name()?.to_string_lossy().to_lowercase();
    let ending = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    if ending == "jps" || ending == "pns" {
        return Some(Eyes::Crossed);
    }
    let words: Vec<&str> = name.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let has = |w: &str| words.contains(&w);
    if has("rl") || has("cross") || has("crosseyed") {
        return Some(Eyes::Crossed);
    }
    if has("sbs") || has("hsbs") || has("lr") || has("3dh") || has("3d") && has("sbs") {
        return Some(Eyes::SideBySide);
    }
    if has("tb") || has("ou") || has("htb") || has("hou") || has("3dv") || has("overunder") {
        return Some(Eyes::TopBottom);
    }
    None
}

/// How alike two halves of a small grey copy of the picture are, -1 to 1, in their detail
/// rather than their light: the sky at the top of both halves of any landscape is alike, and
/// says nothing.
///
/// **Part by part.** The eyes see near things further apart than far ones, so no one shift of
/// the second half matches the first everywhere -- and a phone's two lenses differ a little in
/// exposure, sharpness and how level they sit. Matched whole at its finest detail, the Beam
/// Pro's own 3D photographs scored as two different pictures. So each of twelve parts of the
/// first half finds its own place in the second, a little to either side and a pixel or two up
/// or down, and the middle part's score is the halves'. Detail is taken a step coarser than
/// the pixel: a little blurred, less much blurred.
fn alike(grey: &[f32], width: usize, height: usize, a: (usize, usize), b: (usize, usize), size: (usize, usize)) -> f32 {
    let detail = band(grey, width, height);
    let (w, h) = size;
    let (across, down) = (4usize, 3usize);
    let (pw, ph) = (w / across, h / down);
    if pw < 4 || ph < 4 {
        return -1.0;
    }
    let reach = ((w / 8).max(2) as isize, 2isize);
    let mut parts = Vec::with_capacity(across * down);
    for row in 0..down {
        for col in 0..across {
            let (ax, ay) = (a.0 + col * pw, a.1 + row * ph);
            let mut best = -1.0f32;
            for sy in -reach.1..=reach.1 {
                for sx in -reach.0..=reach.0 {
                    let bx = b.0 as isize + (col * pw) as isize + sx;
                    let by = b.1 as isize + (row * ph) as isize + sy;
                    // Within the other half: its neighbour is not the other eye.
                    if bx < b.0 as isize || by < b.1 as isize || bx as usize + pw > b.0 + w || by as usize + ph > b.1 + h {
                        continue;
                    }
                    let (bx, by) = (bx as usize, by as usize);
                    let (mut ab, mut aa, mut bb) = (0.0f64, 0.0f64, 0.0f64);
                    for y in 0..ph {
                        for x in 0..pw {
                            let p = detail[(ay + y) * width + ax + x] as f64;
                            let q = detail[(by + y) * width + bx + x] as f64;
                            ab += p * q;
                            aa += p * p;
                            bb += q * q;
                        }
                    }
                    if aa > 1e-9 && bb > 1e-9 {
                        best = best.max((ab / (aa * bb).sqrt()) as f32);
                    }
                }
            }
            parts.push(best);
        }
    }
    parts.sort_by(|x, y| x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal));
    parts[parts.len() / 2]
}

/// The detail parts are matched by: a little blurred, less much blurred.
fn band(grey: &[f32], width: usize, height: usize) -> Vec<f32> {
    let near = boxed(grey, width, height, 1);
    let far = boxed(grey, width, height, 3);
    near.iter().zip(&far).map(|(n, f)| n - f).collect()
}

/// Each pixel the average of the square `radius` around it, the edge repeated past the edge.
fn boxed(grey: &[f32], width: usize, height: usize, radius: usize) -> Vec<f32> {
    let r = radius as isize;
    let at = |x: isize, y: isize| grey[(y.clamp(0, height as isize - 1) as usize) * width + x.clamp(0, width as isize - 1) as usize];
    let mut out = vec![0.0f32; width * height];
    let n = ((2 * r + 1) * (2 * r + 1)) as f32;
    for y in 0..height as isize {
        for x in 0..width as isize {
            let mut sum = 0.0f32;
            for dy in -r..=r {
                for dx in -r..=r {
                    sum += at(x + dx, y + dy);
                }
            }
            out[y as usize * width + x as usize] = sum / n;
        }
    }
    out
}

/// The picture as grey, `width` across.
fn grey(rgba: &[u8], from: (u32, u32), width: usize, height: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; width * height];
    let (fw, fh) = (from.0 as usize, from.1 as usize);
    for y in 0..height {
        for x in 0..width {
            // The middle of the patch this pixel stands for: near enough at this size.
            let (sx, sy) = ((x * 2 + 1) * fw / (width * 2), (y * 2 + 1) * fh / (height * 2));
            let i = (sy.min(fh - 1) * fw + sx.min(fw - 1)) * 4;
            out[y * width + x] = (rgba[i] as f32 * 0.3 + rgba[i + 1] as f32 * 0.59 + rgba[i + 2] as f32 * 0.11) / 255.0;
        }
    }
    out
}

/// How alike two halves must be to be taken for two eyes. The Beam Pro's 3D photographs score
/// about 0.9 and an ordinary photograph about 0.15 (2026-11-09).
const ALIKE: f32 = 0.6;

/// How the picture is packed: by its name if that says, by its halves if not.
pub fn guess(path: &Path, width: u32, height: u32, rgba: &[u8]) -> Eyes {
    if let Some(eyes) = by_name(path) {
        return eyes;
    }
    // The Beam Pro's own camera names its 3D photographs and videos SV_ and writes nothing else
    // to say so -- not even its own make -- so a wide one named so is taken at its word: a
    // dark or featureless pair has too little detail to be asked.
    let name = path.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    if name.starts_with("sv_") && width as f32 >= height as f32 * 1.9 {
        return Eyes::SideBySide;
    }
    if width < 64 || height < 64 || rgba.len() < (width * height * 4) as usize {
        return Eyes::Mono;
    }
    let aspect = width as f32 / height as f32;
    // Two pictures side by side are wide -- two portrait views side by side are only half
    // again as wide as tall -- or a normal shape, each squeezed to half; two one over the
    // other are tall, or square.
    if aspect >= 1.4 {
        let (w, h) = (192usize, ((192.0 / aspect) as usize).max(24));
        let grey = grey(rgba, (width, height), w, h);
        if alike(&grey, w, h, (0, 0), (w / 2, 0), (w / 2, h)) > ALIKE {
            return Eyes::SideBySide;
        }
    }
    if aspect <= 1.25 {
        let (w, h) = (96usize, ((96.0 / aspect) as usize).max(24));
        let grey = grey(rgba, (width, height), w, h);
        if alike(&grey, w, h, (0, 0), (0, h / 2), (w, h / 2)) > ALIKE {
            return Eyes::TopBottom;
        }
    }
    Eyes::Mono
}

/// The second image of an MPO -- a 3D camera's pair -- as JPEG bytes, from the index the
/// first image's APP2 "MPF" segment carries (CIPA DC-007).
pub fn mpo_second(bytes: &[u8]) -> Option<&[u8]> {
    if bytes.get(..2)? != [0xFF, 0xD8] {
        return None;
    }
    let mut at = 2usize;
    while at + 4 <= bytes.len() {
        if bytes[at] != 0xFF {
            return None;
        }
        let marker = bytes[at + 1];
        // Image data begins: no MPF segment came before it.
        if marker == 0xDA || marker == 0xD9 {
            return None;
        }
        let length = u16::from_be_bytes([bytes[at + 2], bytes[at + 3]]) as usize;
        let body = bytes.get(at + 4..at + 2 + length)?;
        if marker == 0xE2 && body.starts_with(b"MPF\0") {
            // Offsets in the index count from here: the TIFF header after "MPF\0".
            let base = at + 8;
            return second_from_index(bytes, base, &body[4..]);
        }
        at += 2 + length;
    }
    None
}

fn second_from_index<'a>(bytes: &'a [u8], base: usize, tiff: &[u8]) -> Option<&'a [u8]> {
    let little = match tiff.get(..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let u16_at = |i: usize| -> Option<u16> {
        let b = tiff.get(i..i + 2)?;
        Some(if little { u16::from_le_bytes([b[0], b[1]]) } else { u16::from_be_bytes([b[0], b[1]]) })
    };
    let u32_at = |i: usize| -> Option<u32> {
        let b = tiff.get(i..i + 4)?;
        Some(if little { u32::from_le_bytes([b[0], b[1], b[2], b[3]]) } else { u32::from_be_bytes([b[0], b[1], b[2], b[3]]) })
    };
    let ifd = u32_at(4)? as usize;
    let count = u16_at(ifd)? as usize;
    for entry in 0..count {
        let e = ifd + 2 + entry * 12;
        // MPEntry: sixteen bytes an image -- attributes, size, offset, two dependents.
        if u16_at(e)? == 0xB002 {
            let entries = u32_at(e + 4)? as usize / 16;
            let list = u32_at(e + 8)? as usize;
            if entries < 2 {
                return None;
            }
            let second = list + 16;
            let size = u32_at(second + 4)? as usize;
            let offset = u32_at(second + 8)? as usize;
            let start = base + offset;
            let image = bytes.get(start..start + size)?;
            return (image.get(..2) == Some(&[0xFF, 0xD8])).then_some(image);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A picture of scattered dots, which has detail everywhere and no two parts alike.
    fn noise(width: u32, height: u32, seed: u32) -> Vec<u8> {
        let mut state = seed.wrapping_mul(2654435761).max(1);
        let mut rgba = Vec::with_capacity((width * height * 4) as usize);
        for _ in 0..width * height {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let v = (state >> 24) as u8;
            rgba.extend_from_slice(&[v, v, v, 255]);
        }
        rgba
    }

    /// A blurred copy, so that the picture has detail at the scale it is looked at.
    fn blobs(width: u32, height: u32, seed: u32) -> Vec<u8> {
        let fine = noise(width / 8 + 1, height / 8 + 1, seed);
        let fw = width / 8 + 1;
        let mut rgba = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                let v = fine[(((y / 8) * fw + x / 8) * 4) as usize];
                rgba.extend_from_slice(&[v, v, v, 255]);
            }
        }
        rgba
    }

    fn side_by_side(eye: &[u8], width: u32, height: u32, shift: u32) -> Vec<u8> {
        let mut rgba = Vec::with_capacity((width * height * 8) as usize);
        for y in 0..height {
            let row = &eye[(y * width * 4) as usize..((y + 1) * width * 4) as usize];
            rgba.extend_from_slice(row);
            // The right eye sees the scene a little to one side.
            let mut shifted = row[(shift * 4) as usize..].to_vec();
            shifted.extend_from_slice(&row[..(shift * 4) as usize]);
            rgba.extend_from_slice(&shifted);
        }
        rgba
    }

    #[test]
    fn two_views_of_one_scene_side_by_side_are_known_as_such() {
        let eye = blobs(640, 480, 7);
        let pair = side_by_side(&eye, 640, 480, 12);
        assert_eq!(guess(Path::new("IMG_0001.jpg"), 1280, 480, &pair), Eyes::SideBySide);
    }

    #[test]
    fn a_wide_picture_of_one_thing_is_not() {
        let wide = blobs(1280, 480, 3);
        assert_eq!(guess(Path::new("panorama.jpg"), 1280, 480, &wide), Eyes::Mono);
        let ordinary = blobs(800, 600, 5);
        assert_eq!(guess(Path::new("cat.jpg"), 800, 600, &ordinary), Eyes::Mono);
    }

    #[test]
    fn one_over_the_other_is_known() {
        let eye = blobs(640, 360, 11);
        let mut pair = eye.clone();
        pair.extend_from_slice(&eye);
        assert_eq!(guess(Path::new("x.png"), 640, 720, &pair), Eyes::TopBottom);
    }

    #[test]
    fn names_say_what_they_say() {
        let flat = vec![0u8; 4 * 100 * 100];
        assert_eq!(guess(Path::new("holiday.jps"), 100, 100, &flat), Eyes::Crossed);
        assert_eq!(guess(Path::new("beach_SBS.jpg"), 100, 100, &flat), Eyes::SideBySide);
        assert_eq!(guess(Path::new("tower.3d.tb.png"), 100, 100, &flat), Eyes::TopBottom);
        assert_eq!(guess(Path::new("subscription.png"), 100, 100, &flat), Eyes::Mono);
    }

    #[test]
    fn an_mpo_gives_up_its_second_image() {
        // The smallest thing that is an MPO: an SOI, an APP2 MPF index of two entries, and a
        // second "image" that is an SOI and an EOI.
        let mut tiff = Vec::new();
        tiff.extend_from_slice(b"II*\0");
        tiff.extend_from_slice(&8u32.to_le_bytes());
        tiff.extend_from_slice(&1u16.to_le_bytes());
        // MPEntry: type UNDEFINED (7), 32 bytes, at offset 26.
        tiff.extend_from_slice(&0xB002u16.to_le_bytes());
        tiff.extend_from_slice(&7u16.to_le_bytes());
        tiff.extend_from_slice(&32u32.to_le_bytes());
        tiff.extend_from_slice(&26u32.to_le_bytes());
        tiff.extend_from_slice(&0u32.to_le_bytes());
        assert_eq!(tiff.len(), 26);
        let segment_length = 2 + 4 + tiff.len() + 32;
        let base = 2 + 4 + 4;
        let file_length = base + tiff.len() + 32 + 2;
        let second_at = file_length - base;
        // First entry: the first image. Second: four bytes at `second_at`.
        for (size, offset) in [(file_length as u32, 0u32), (4, second_at as u32)] {
            tiff.extend_from_slice(&0u32.to_le_bytes());
            tiff.extend_from_slice(&size.to_le_bytes());
            tiff.extend_from_slice(&offset.to_le_bytes());
            tiff.extend_from_slice(&0u32.to_le_bytes());
        }
        let mut bytes = vec![0xFF, 0xD8, 0xFF, 0xE2];
        bytes.extend_from_slice(&(segment_length as u16).to_be_bytes());
        bytes.extend_from_slice(b"MPF\0");
        bytes.extend_from_slice(&tiff);
        bytes.extend_from_slice(&[0xFF, 0xD9]);
        bytes.extend_from_slice(&[0xFF, 0xD8, 0xFF, 0xD9]);
        assert_eq!(mpo_second(&bytes), Some(&[0xFF, 0xD8, 0xFF, 0xD9][..]));
    }
}
