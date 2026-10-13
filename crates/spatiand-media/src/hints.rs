//! Works out what a file is from its name, and from whatever the container admitted.
//!
//! Ported from `app/src/main/java/id/prasetya/vibrefy/app/player/VideoLayoutHints.kt`.
//!
//! Neither source is reliable on its own. Spatial-media metadata is precise but almost
//! always missing - the `st3d`/`sv3d` boxes only appear on files produced by tools that
//! write them, which rules out every 3D rip in circulation. Those instead carry the
//! marking in the filename, which is exact enough in practice because the conventions are
//! narrow: `HSBS`, `3DH`, `VR180`, `_360_TB`.
//!
//! So both are read, and **the name wins where the two disagree**. Not because it is more
//! trustworthy in the abstract, but because it is the one the viewer can correct: renaming
//! a file takes a moment, rewriting its boxes takes a tool. The container then fills in
//! whatever the name did not mention.
//!
//! This carries more weight here than it did on Android. Vibrefy sends no video metadata
//! whatsoever - no codec, no resolution, no duration, and nothing at all about stereo or
//! projection - so until the engine has opened the file, the name is the only evidence
//! there is. Everything here is still a starting guess; the player offers the same choices
//! by hand, and that override is the real answer to a wrong reading.

use crate::layout::{Packing, Projection, StereoLayout, VideoLayout};

/// The container's declared stereo mode, kept as plain values so this module stays free of
/// any media engine and testable without one. Map the engine's own vocabulary onto these
/// at the edge - see [`ContainerStereo::from_ffmpeg`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ContainerStereo {
    #[default]
    Unknown,
    Mono,
    TopBottom,
    LeftRight,
}

impl ContainerStereo {
    /// FFmpeg's `AVStereo3DType`, which is what a container's own marking arrives as: the
    /// Matroska `StereoMode`, MP4's `st3d`, an H.264 frame-packing message.
    ///
    /// Only the packing is taken; which eye leads is a separate control the viewer can
    /// reach, because a file that says the right eye is first and means the left is common
    /// enough that believing it blindly would be worse than offering a switch.
    ///
    /// Everything else it can say - the checkerboard, column and row interleaves, frames in
    /// turn - is for displays this player will never drive, and reads as unknown rather than
    /// as a guess.
    pub fn from_ffmpeg(kind: u32) -> Self {
        // AV_STEREO3D_2D, _SIDEBYSIDE, _TOPBOTTOM: fixed by FFmpeg's ABI.
        match kind {
            0 => Self::Mono,
            1 => Self::LeftRight,
            2 => Self::TopBottom,
            _ => Self::Unknown,
        }
    }
}

/// Everything the file itself was willing to say. All of it optional, because on this
/// server all of it usually is.
#[derive(Debug, Clone, Copy, Default)]
pub struct ContainerHints {
    pub stereo: ContainerStereo,
    /// A projection box of any kind. Equirectangular 360 is the only one this renderer
    /// draws, and the overwhelming majority of what exists.
    ///
    /// FFmpeg reads MP4's `sv3d` and Matroska's projection element into spherical side
    /// data, so this is true for a file that carries either. They are almost never present
    /// on anything in circulation, which is why the name is still the usual evidence.
    pub has_projection_data: bool,
    /// That data says the picture is the front half of a sphere, not all of one.
    pub projection_is_half: bool,
    pub frame_width: u32,
    pub frame_height: u32,
}

pub fn detect(file_name: &str, container: ContainerHints) -> VideoLayout {
    let name = Name::new(file_name);

    let named_stereo = stereo_of(&name);
    let declared_stereo = named_stereo.or(match container.stereo {
        ContainerStereo::TopBottom => Some(StereoLayout::OverUnder),
        ContainerStereo::LeftRight => Some(StereoLayout::SideBySide),
        _ => None,
    });

    let projection = projection_of(&name).unwrap_or(if container.projection_is_half {
        Projection::Dome180
    } else if container.has_projection_data {
        Projection::Sphere360
    } else {
        Projection::Flat
    });

    let stereo = declared_stereo
        .or_else(|| spherical_stereo(projection, container.frame_width, container.frame_height))
        .unwrap_or(StereoLayout::Mono);

    VideoLayout {
        projection,
        stereo,
        // Spatial-media packing is always full-frame, so a container-declared pairing needs
        // no guessing. A name-declared one does, unless the name said which, and one read
        // off a sphere's proportions does not care either way.
        packing: packing_of(&name).unwrap_or({
            if named_stereo.is_none() && declared_stereo.is_some() {
                Packing::Full
            } else {
                Packing::Auto
            }
        }),
    }
}

/// How a spherical file pairs its eyes, read off the frame's shape.
///
/// Names like `VR180` say where the picture goes but nothing about how it is packed -
/// and a 180 file left as mono has both eyes wrapped round the dome at once, which is
/// unmistakable and unwatchable. The shape settles it, because equirectangular fixes the
/// proportions: one eye of 180 degrees covers 180 by 180 and so is square, one eye of
/// 360 covers 360 by 180 and so is 2:1. Pairing them side by side doubles the width,
/// stacking them doubles the height, and the three possibilities are an octave apart -
/// far too far to confuse.
fn spherical_stereo(projection: Projection, width: u32, height: u32) -> Option<StereoLayout> {
    if projection == Projection::Flat || width == 0 || height == 0 {
        return None;
    }
    let aspect = f64::from(width) / f64::from(height);
    let mono = if projection == Projection::Sphere360 {
        2.0
    } else {
        1.0
    };
    [
        (mono * 2.0, StereoLayout::SideBySide),
        (mono, StereoLayout::Mono),
        (mono / 2.0, StereoLayout::OverUnder),
    ]
    .into_iter()
    .min_by(|(a, _), (b, _)| {
        (aspect / a).ln().abs().total_cmp(&(aspect / b).ln().abs())
    })
    .map(|(_, layout)| layout)
}

fn stereo_of(name: &Name) -> Option<StereoLayout> {
    if name.any(&["hsbs", "fsbs", "sbs", "3dh", "sbs3d", "3dsbs"])
        || name.phrase(&["h.sbs", "f.sbs", "half.sbs", "full.sbs", "side.by.side"])
    {
        return Some(StereoLayout::SideBySide);
    }
    if name.any(&["hou", "htb", "fou", "3dv", "ou3d"])
        || name.phrase(&["h.ou", "h.tb", "half.ou", "half.tb", "full.ou", "full.tb"])
        || name.any(&["overunder", "topbottom"])
        || name.phrase(&["over.under", "top.bottom"])
    {
        return Some(StereoLayout::OverUnder);
    }
    // "ou", "tb" and "lr" are ordinary enough as words and initials that they only count
    // once something else has already said this is a 3D or spatial file.
    if name.spatial && name.any(&["ou", "tb"]) {
        return Some(StereoLayout::OverUnder);
    }
    if name.spatial && name.any(&["lr"]) {
        return Some(StereoLayout::SideBySide);
    }
    None
}

fn projection_of(name: &Name) -> Option<Projection> {
    // 360x180 is a full sphere written out in both dimensions, so 360 has to be asked
    // about first or it reads as a dome.
    if name.number(360) || name.any(&["vr360", "360vr", "equirect", "equirectangular"]) {
        return Some(Projection::Sphere360);
    }
    if name.number(180) || name.any(&["vr180", "180vr"]) {
        return Some(Projection::Dome180);
    }
    None
}

fn packing_of(name: &Name) -> Option<Packing> {
    if name.any(&["hsbs", "hou", "htb"])
        || name.phrase(&[
            "h.sbs",
            "h.ou",
            "h.tb",
            "half.sbs",
            "half.ou",
            "half.tb",
            "half.over.under",
        ])
    {
        return Some(Packing::Half);
    }
    if name.any(&["fsbs", "fou"])
        || name.phrase(&["f.sbs", "full.sbs", "full.ou", "full.tb", "full.over.under"])
    {
        return Some(Packing::Full);
    }
    None
}

/// A filename reduced to the two forms worth asking questions of.
///
/// `tokens` are the words, split on anything that is not a letter or digit, which is what
/// makes `hsbs` and `half-sbs` and `Half.SBS` the same question. `numbers` are the digit
/// runs *inside* those words, so `180x180` offers up 180 while `1080p` does not offer up
/// 180 - the trap that makes a naive search for "180" useless on release names.
struct Name {
    normalised: String,
    tokens: Vec<String>,
    numbers: Vec<u32>,
    spatial: bool,
}

impl Name {
    fn new(raw: &str) -> Self {
        let lowered = raw.to_lowercase();
        let mut normalised = String::with_capacity(lowered.len() + 2);
        normalised.push('.');
        let mut last_was_separator = true;
        for ch in lowered.chars() {
            if ch.is_ascii_alphanumeric() {
                normalised.push(ch);
                last_was_separator = false;
            } else if !last_was_separator {
                normalised.push('.');
                last_was_separator = true;
            }
        }
        if !normalised.ends_with('.') {
            normalised.push('.');
        }

        let tokens: Vec<String> = normalised
            .split('.')
            .filter(|t| !t.is_empty())
            .map(str::to_string)
            .collect();

        let mut numbers = Vec::new();
        for token in &tokens {
            let mut run = String::new();
            for ch in token.chars() {
                if ch.is_ascii_digit() {
                    run.push(ch);
                } else if !run.is_empty() {
                    if let Ok(value) = run.parse() {
                        numbers.push(value);
                    }
                    run.clear();
                }
            }
            if !run.is_empty() {
                if let Ok(value) = run.parse() {
                    numbers.push(value);
                }
            }
        }

        let mut name = Self {
            normalised,
            tokens,
            numbers,
            spatial: false,
        };
        // Enough has been said already that a two-letter marker can be taken at its word.
        name.spatial = name.any(&[
            "3d",
            "vr",
            "stereo",
            "stereoscopic",
            "spatial",
            "vr180",
            "vr360",
            "equirect",
        ]) || name.number(180)
            || name.number(360);
        name
    }

    fn any(&self, words: &[&str]) -> bool {
        words.iter().any(|w| self.tokens.iter().any(|t| t == w))
    }

    fn phrase(&self, phrases: &[&str]) -> bool {
        phrases
            .iter()
            .any(|p| self.normalised.contains(&format!(".{p}.")))
    }

    fn number(&self, value: u32) -> bool {
        self.numbers.contains(&value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape(name: &str) -> VideoLayout {
        detect(name, ContainerHints::default())
    }

    fn sized(name: &str, w: u32, h: u32) -> VideoLayout {
        detect(
            name,
            ContainerHints {
                frame_width: w,
                frame_height: h,
                ..ContainerHints::default()
            },
        )
    }

    #[test]
    fn an_ordinary_release_is_left_alone() {
        for name in [
            "The Batman (2022) 1080p HEVC EAC3.mp4",
            "Show.s01e01.2160p.x265.mp4",
            "Blade Runner 2049 [1920x1080] DTS-HD 5.1.mp4",
            "Apollo 13 (1995) 720p AC3.mp4",
        ] {
            assert_eq!(VideoLayout::FLAT_MONO, shape(name), "misread {name}");
        }
    }

    /// The resolution trap: a plain search for "180" finds it inside every 1080p file in
    /// the library, and a plain search for two-letter layout codes finds them inside words.
    #[test]
    fn resolutions_and_ordinary_words_are_not_layout_markings() {
        assert_eq!(Projection::Flat, shape("Doc.1080p.mp4").projection);
        assert_eq!(Projection::Flat, shape("Doc.1920x1080.mp4").projection);
        assert_eq!(Projection::Flat, shape("Doc.3840x2160.mp4").projection);
        assert_eq!(StereoLayout::Mono, shape("Toi ou Moi (2019).mp4").stereo);
        assert_eq!(StereoLayout::Mono, shape("Spy Kids 3D.mp4").stereo);
    }

    #[test]
    fn half_side_by_side_is_read_from_the_usual_markings() {
        for name in [
            "Avatar.2009.1080p.HSBS.mp4",
            "Avatar.2009.Half-SBS.mp4",
            "Avatar.H.SBS.mp4",
        ] {
            assert_eq!(StereoLayout::SideBySide, shape(name).stereo, "{name}");
            assert_eq!(Packing::Half, shape(name).packing, "{name}");
        }
    }

    #[test]
    fn over_under_is_read_from_the_usual_markings() {
        for name in ["Gravity.3D.Half-OU.mp4", "Gravity.HOU.mp4", "Gravity.Over-Under.mp4"] {
            assert_eq!(StereoLayout::OverUnder, shape(name).stereo, "{name}");
        }
    }

    #[test]
    fn full_width_pairings_say_so() {
        assert_eq!(Packing::Full, shape("Avatar.FSBS.mp4").packing);
        assert_eq!(Packing::Full, shape("Avatar.Full-SBS.mp4").packing);
    }

    /// Nothing said which, so the frame gets to answer - see the `layout` tests.
    #[test]
    fn an_unqualified_pairing_leaves_the_packing_open() {
        assert_eq!(
            Packing::Auto,
            shape("Avengers.2012.1080p.3D.SBS.mp4").packing
        );
    }

    #[test]
    fn spherical_files_are_recognised_and_360_by_180_is_a_whole_sphere() {
        assert_eq!(Projection::Dome180, shape("Concert_VR180_LR.mp4").projection);
        assert_eq!(Projection::Dome180, shape("clip_180x180_3dh.mp4").projection);
        assert_eq!(Projection::Sphere360, shape("Beach_360_TB.mp4").projection);
        assert_eq!(Projection::Sphere360, shape("Doc.360x180.mp4").projection);
    }

    /// Two letters are only enough once the name has already declared itself spatial.
    #[test]
    fn short_layout_codes_count_only_alongside_a_spatial_marking() {
        assert_eq!(StereoLayout::SideBySide, shape("Concert_VR180_LR.mp4").stereo);
        assert_eq!(StereoLayout::OverUnder, shape("Beach_360_TB.mp4").stereo);
        assert_eq!(StereoLayout::Mono, shape("Live at the LR Theatre.mp4").stereo);
    }

    /// `VR180` says where the picture goes, not how its eyes are packed - and a stereo file
    /// left as mono wraps both eyes round the dome at once. Equirectangular proportions
    /// settle it: one 180 eye is square, one 360 eye is 2:1, and pairing doubles one side.
    #[test]
    fn a_spherical_files_pairing_is_read_from_its_proportions() {
        assert_eq!(
            StereoLayout::SideBySide,
            sized("Demo 3D VR180.mp4", 3840, 1920).stereo
        );
        assert_eq!(StereoLayout::Mono, sized("Demo 3D VR180.mp4", 1920, 1920).stereo);
        assert_eq!(
            StereoLayout::OverUnder,
            sized("Demo 3D VR180.mp4", 1920, 3840).stereo
        );

        assert_eq!(StereoLayout::Mono, sized("Beach 360.mp4", 3840, 1920).stereo);
        assert_eq!(
            StereoLayout::OverUnder,
            sized("Beach 360.mp4", 3840, 3840).stereo
        );
        assert_eq!(
            StereoLayout::SideBySide,
            sized("Beach 360.mp4", 7680, 1920).stereo
        );
    }

    /// Proportions are the fallback, not the authority: a stated pairing still wins.
    #[test]
    fn a_named_pairing_survives_a_frame_that_disagrees() {
        assert_eq!(
            StereoLayout::OverUnder,
            sized("Trip_360_TB.mp4", 3840, 1920).stereo
        );
    }

    /// Nothing to measure, and a flat file has no proportions worth reading this way.
    #[test]
    fn an_ordinary_file_is_unaffected_by_its_frame_size() {
        assert_eq!(
            VideoLayout::FLAT_MONO,
            sized("The Batman (2022) 1080p.mp4", 3840, 1920)
        );
    }

    #[test]
    fn the_container_is_believed_when_the_name_says_nothing() {
        let declared = detect(
            "Holiday.mp4",
            ContainerHints {
                stereo: ContainerStereo::LeftRight,
                ..ContainerHints::default()
            },
        );
        assert_eq!(StereoLayout::SideBySide, declared.stereo);
        // Spatial-media pairings are full-frame by definition, so there is nothing to infer.
        assert_eq!(Packing::Full, declared.packing);

        assert_eq!(
            Projection::Sphere360,
            detect(
                "Holiday.mp4",
                ContainerHints {
                    has_projection_data: true,
                    ..ContainerHints::default()
                },
            )
            .projection
        );
    }

    /// FFmpeg's own numbers, mapped onto ours.
    #[test]
    fn ffmpegs_stereo_kinds_are_understood() {
        assert_eq!(ContainerStereo::Mono, ContainerStereo::from_ffmpeg(0));
        assert_eq!(ContainerStereo::LeftRight, ContainerStereo::from_ffmpeg(1));
        assert_eq!(ContainerStereo::TopBottom, ContainerStereo::from_ffmpeg(2));
        // Frame sequence, checkerboard, lines, columns: not ours to show.
        for kind in 3..=8 {
            assert_eq!(ContainerStereo::Unknown, ContainerStereo::from_ffmpeg(kind), "{kind}");
        }
    }

    /// A container can be tagged wrong, and re-tagging it needs a tool. Renaming the file
    /// does not, so the name is the override of last resort and has to win.
    #[test]
    fn the_name_overrules_a_container_that_disagrees() {
        let layout = detect(
            "Gravity.HOU.mp4",
            ContainerHints {
                stereo: ContainerStereo::LeftRight,
                ..ContainerHints::default()
            },
        );
        assert_eq!(StereoLayout::OverUnder, layout.stereo);
        assert_eq!(Packing::Half, layout.packing);
    }
}
