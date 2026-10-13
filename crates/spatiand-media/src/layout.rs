//! What a file is, as far as the renderer is concerned.
//!
//! From the Vibrefy Rust port (`vibrefy-media`), itself ported from the Android app's
//! `player/VideoLayout.kt`.
//!
//! One thing is deliberately *not* carried across: the Android app's `StereoOutput.FOLLOW`
//! and its `GlassesMode` probe. There, the app had to work out for itself what the XREAL
//! glasses were doing and pack a side-by-side frame to suit. Under Spatiand the compositor
//! owns both eyes, so the client's job ends at declaring a layout - see
//! `spatiand_stereo_v1` in `docs/apps.md`. What is left is the flat-screen question of
//! which eye to show when nothing can show two.

/// How the frame maps onto geometry: a screen, or the inside of a sphere.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Projection {
    #[default]
    Flat,
    /// Equirectangular hemisphere - 180 degrees of longitude in front of the viewer.
    Dome180,
    /// Equirectangular sphere - the full 360 by 180.
    Sphere360,
}

impl Projection {
    pub fn label(self) -> &'static str {
        match self {
            Self::Flat => "Flat screen",
            Self::Dome180 => "180° dome",
            Self::Sphere360 => "360° sphere",
        }
    }
}

/// How the two eyes share one frame.
///
/// Labels stay short enough to sit on one line; "In the file" above them carries the rest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StereoLayout {
    #[default]
    Mono,
    SideBySide,
    OverUnder,
}

impl StereoLayout {
    pub fn label(self) -> &'static str {
        match self {
            Self::Mono => "One picture",
            Self::SideBySide => "Side by side",
            Self::OverUnder => "Over / under",
        }
    }
}

/// Whether a packed frame was squeezed to fit.
///
/// Full packing gives each eye its own full-size image, so a 1080p pair arrives as
/// 3840x1080. Half packing keeps the frame at 1920x1080 and squashes each eye into
/// 960x1080, to be stretched back out on display. Both are common, and nothing in the
/// container distinguishes them - see [`VideoLayout::eye_aspect`] for how [`Packing::Auto`]
/// decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Packing {
    #[default]
    Auto,
    Half,
    Full,
}

impl Packing {
    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Work it out",
            Self::Half => "Squeezed to half",
            Self::Full => "Full width",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Eye {
    Left,
    Right,
}

/// How the picture reaches the viewer's eyes on a surface that cannot show two of them.
///
/// Under Spatiand with `spatiand_stereo_v1` bound this is not consulted at all: the
/// layout is declared and the compositor samples each eye's half itself. It is the answer
/// for an ordinary monitor, and for Spatiand before that protocol exists - where
/// `docs/apps.md` says in as many words to ship mono.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StereoOutput {
    /// One eye, filling the surface. A 3D file watched flat, and the only sane view of a
    /// 2D one.
    #[default]
    SingleEye,
    /// Left and right halves of the surface, for a compositor that gives each eye one half
    /// stretched across the whole surface - Spatiand, with the window declared side by side.
    /// Each half is drawn squeezed by two to survive the stretch.
    SideBySide,
    /// The frame goes over exactly as it arrived, packed, for something downstream to take
    /// apart.
    ///
    /// This is what `spatiand_stereo_v1` wants: the compositor samples `u ∈ [0, 0.5]` for
    /// one eye and `[0.5, 1]` for the other, and `docs/apps.md` says to "send whatever your
    /// source is; do not unsqueeze it yourself". Unpacking here and letting the compositor
    /// unpack again would give a squashed picture in both eyes.
    Packed,
}

impl StereoOutput {
    pub fn label(self) -> &'static str {
        match self {
            Self::SingleEye => "One eye",
            Self::SideBySide => "Side-by-side 3D",
            Self::Packed => "Both, packed",
        }
    }
}

/// What a file is.
///
/// Deliberately separate from how it gets *shown*: the same 3D file is one thing whether
/// it is being watched flat in a window or wrapped round the room.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct VideoLayout {
    pub projection: Projection,
    pub stereo: StereoLayout,
    pub packing: Packing,
}

/// Below this, one side-by-side eye is too square to be anything but squeezed.
const SQUARISH: f32 = 1.3;

/// Above this, one over-under eye is too wide to be anything but squeezed.
const STRETCHED: f32 = 3.0;

impl VideoLayout {
    pub const FLAT_MONO: Self = Self {
        projection: Projection::Flat,
        stereo: StereoLayout::Mono,
        packing: Packing::Auto,
    };

    pub fn is_stereo(&self) -> bool {
        self.stereo != StereoLayout::Mono
    }

    pub fn is_spherical(&self) -> bool {
        self.projection != Projection::Flat
    }

    /// The half of the frame belonging to one eye, as `(u, v, width, height)` in texture
    /// coordinates with the origin at the top left.
    ///
    /// For over-under the first eye is the top half, which is what both the Matroska
    /// `StereoMode` and the spatial-media `st3d` conventions mean by it - and, usefully,
    /// what Spatiand's own sky shader means by `over_under` too.
    pub fn eye_rect(&self, eye: Eye) -> [f32; 4] {
        match (self.stereo, eye) {
            (StereoLayout::Mono, _) => [0.0, 0.0, 1.0, 1.0],
            (StereoLayout::SideBySide, Eye::Left) => [0.0, 0.0, 0.5, 1.0],
            (StereoLayout::SideBySide, Eye::Right) => [0.5, 0.0, 0.5, 1.0],
            (StereoLayout::OverUnder, Eye::Left) => [0.0, 0.0, 1.0, 0.5],
            (StereoLayout::OverUnder, Eye::Right) => [0.0, 0.5, 1.0, 0.5],
        }
    }

    /// The shape one eye's picture should be drawn at, given the decoded frame size.
    ///
    /// Only [`Projection::Flat`] asks: on a sphere the mapping is angular and the frame's
    /// own proportions do not survive it.
    ///
    /// [`Packing::Auto`] reads the answer off the frame, because a half-packed eye is
    /// distorted in a direction no real film is. Halving 16:9 side by side gives 0.89 -
    /// squarer than any release - and halving a 2.39:1 scope frame still only reaches
    /// 1.19, while the narrowest *full* pairing, 4:3, sits at 1.33. Over-under is the same
    /// test upside down: a half-packed eye comes out wider than 3:1, which no full pairing
    /// reaches.
    pub fn eye_aspect(&self, frame_width: u32, frame_height: u32) -> f32 {
        if frame_width == 0 || frame_height == 0 {
            return 16.0 / 9.0;
        }
        let frame = frame_width as f32 / frame_height as f32;
        match self.stereo {
            StereoLayout::Mono => frame,
            StereoLayout::SideBySide => {
                if self.is_half(frame / 2.0 < SQUARISH) {
                    frame
                } else {
                    frame / 2.0
                }
            }
            StereoLayout::OverUnder => {
                if self.is_half(frame * 2.0 > STRETCHED) {
                    frame
                } else {
                    frame * 2.0
                }
            }
        }
    }

    fn is_half(&self, guess: bool) -> bool {
        match self.packing {
            Packing::Half => true,
            Packing::Full => false,
            Packing::Auto => guess,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sbs(packing: Packing) -> VideoLayout {
        VideoLayout {
            stereo: StereoLayout::SideBySide,
            packing,
            ..VideoLayout::FLAT_MONO
        }
    }

    fn over_under(packing: Packing) -> VideoLayout {
        VideoLayout {
            stereo: StereoLayout::OverUnder,
            packing,
            ..VideoLayout::FLAT_MONO
        }
    }

    #[track_caller]
    fn assert_aspect(expected: f32, actual: f32) {
        assert!(
            (expected - actual).abs() < 0.01,
            "expected {expected}, got {actual}"
        );
    }

    #[test]
    fn a_plain_frame_is_its_own_shape() {
        assert_aspect(1.78, VideoLayout::FLAT_MONO.eye_aspect(1920, 1080));
        assert_aspect(2.39, VideoLayout::FLAT_MONO.eye_aspect(1920, 804));
    }

    /// Both of these are a 16:9 film. One was squeezed to fit a 16:9 frame, the other was
    /// not, and only the numbers say which.
    #[test]
    fn side_by_side_comes_out_16_by_9_whether_or_not_it_was_squeezed() {
        assert_aspect(1.78, sbs(Packing::Auto).eye_aspect(1920, 1080));
        assert_aspect(1.78, sbs(Packing::Auto).eye_aspect(3840, 1080));
    }

    /// Scope is where the guess earns its keep: half of a 2.39:1 frame is still wider than
    /// tall, so "too square to be real" would miss it. The threshold sits above it instead.
    #[test]
    fn a_squeezed_scope_frame_is_not_mistaken_for_a_full_width_pair() {
        assert_aspect(2.39, sbs(Packing::Auto).eye_aspect(1920, 804));
        assert_aspect(2.39, sbs(Packing::Auto).eye_aspect(5120, 1072));
    }

    #[test]
    fn over_under_is_the_same_question_upside_down() {
        assert_aspect(1.78, over_under(Packing::Auto).eye_aspect(1920, 1080));
        assert_aspect(1.78, over_under(Packing::Auto).eye_aspect(1920, 2160));
        assert_aspect(2.39, over_under(Packing::Auto).eye_aspect(1920, 804));
        assert_aspect(2.39, over_under(Packing::Auto).eye_aspect(1920, 1608));
    }

    /// 4:3 is the narrowest thing that arrives full-width, and must survive the threshold.
    #[test]
    fn a_full_width_four_by_three_pair_is_not_read_as_squeezed() {
        assert_aspect(1.33, sbs(Packing::Auto).eye_aspect(2880, 1080));
    }

    /// Both of these are frames the guess would have read the other way, and both come out
    /// distorted - which is the point. An override is only worth having if it can be wrong.
    #[test]
    fn being_told_beats_working_it_out() {
        assert_aspect(0.89, sbs(Packing::Full).eye_aspect(1920, 1080));
        assert_aspect(3.56, sbs(Packing::Half).eye_aspect(3840, 1080));
    }

    #[test]
    fn nothing_to_divide_by_is_answered_rather_than_thrown() {
        assert_aspect(1.78, sbs(Packing::Auto).eye_aspect(0, 0));
    }

    #[test]
    fn each_eye_gets_its_own_half_and_over_under_puts_the_left_one_on_top() {
        assert_eq!([0.0, 0.0, 0.5, 1.0], sbs(Packing::Auto).eye_rect(Eye::Left));
        assert_eq!([0.5, 0.0, 0.5, 1.0], sbs(Packing::Auto).eye_rect(Eye::Right));
        assert_eq!(
            [0.0, 0.0, 1.0, 0.5],
            over_under(Packing::Auto).eye_rect(Eye::Left)
        );
        assert_eq!(
            [0.0, 0.5, 1.0, 0.5],
            over_under(Packing::Auto).eye_rect(Eye::Right)
        );
    }

    #[test]
    fn a_2d_file_hands_the_whole_frame_to_both_eyes() {
        assert_eq!([0.0, 0.0, 1.0, 1.0], VideoLayout::FLAT_MONO.eye_rect(Eye::Left));
        assert_eq!(
            [0.0, 0.0, 1.0, 1.0],
            VideoLayout::FLAT_MONO.eye_rect(Eye::Right)
        );
    }
}
