//! Which films this machine decodes with its own hardware.
//!
//! A web page offered a choice of formats takes the one it is told is best, and a format this
//! machine can only decode in software is several cores' worth of work a film -- an Intel Mac
//! given YouTube's AV1 three times over was all but unusable. So the pages are told only of what
//! is decoded in hardware, and the page picks something that is.

/// Whether a codec, as a MIME `codecs` parameter names it (`avc1.64001e`, `av01.0.08M.08`,
/// `vp09.00.40.08`, `hvc1.1.6.L93.B0`), is decoded in hardware here. Codecs that are cheap in
/// software anyway (sound, VP8) are always yes.
pub fn decoded_in_hardware(codec: &str) -> bool {
    let name = codec.trim().split('.').next().unwrap_or("").to_ascii_lowercase();
    let family = match name.as_str() {
        "avc1" | "avc3" => Family::H264,
        "hev1" | "hvc1" | "dvh1" | "dvhe" => Family::Hevc,
        "vp09" | "vp9" => Family::Vp9,
        "av01" | "av1" => Family::Av1,
        _ => return true,
    };
    platform::decodes(family)
}

/// The video codec families pages are offered a choice among.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Family {
    H264,
    Hevc,
    Vp9,
    Av1,
}

/// Each family this machine cannot decode in hardware, by the `codecs` names pages use for it.
pub fn software_only() -> Vec<&'static str> {
    let mut out = Vec::new();
    for (family, names) in [
        (Family::H264, &["avc1", "avc3"][..]),
        (Family::Hevc, &["hev1", "hvc1", "dvh1", "dvhe"][..]),
        (Family::Vp9, &["vp09", "vp9"][..]),
        (Family::Av1, &["av01", "av1"][..]),
    ] {
        // A Mac's VP9 (and AV1) decoders are supplemental, run in a service of their own; given
        // a MediaSource's pieces, VP9's makes one picture and never answers again (2026-10-18,
        // an Intel Mac, YouTube), and a second film opened meanwhile waits on it. Its H.264 is
        // the GPU's own and takes them whole, so pages here are told VP9 is not to be had.
        let stalls = cfg!(target_os = "macos") && matches!(family, Family::Vp9 | Family::Av1);
        if !platform::decodes(family) || stalls {
            out.extend_from_slice(names);
        }
    }
    out
}

#[cfg(target_os = "macos")]
mod platform {
    use super::Family;
    use std::sync::OnceLock;

    #[link(name = "VideoToolbox", kind = "framework")]
    extern "C" {
        fn VTIsHardwareDecodeSupported(codec_type: u32) -> u8;
        fn VTRegisterSupplementalVideoDecoderIfAvailable(codec_type: u32);
    }

    const fn fourcc(s: &[u8; 4]) -> u32 {
        (s[0] as u32) << 24 | (s[1] as u32) << 16 | (s[2] as u32) << 8 | s[3] as u32
    }

    /// VideoToolbox's own answer, asked once each. VP9 and AV1 are supplemental decoders on
    /// a Mac, there only once asked for.
    pub fn decodes(family: Family) -> bool {
        static ANSWERS: OnceLock<[bool; 4]> = OnceLock::new();
        let answers = ANSWERS.get_or_init(|| unsafe {
            let vp9 = fourcc(b"vp09");
            let av1 = fourcc(b"av01");
            VTRegisterSupplementalVideoDecoderIfAvailable(vp9);
            VTRegisterSupplementalVideoDecoderIfAvailable(av1);
            let answers = [
                VTIsHardwareDecodeSupported(fourcc(b"avc1")) != 0,
                VTIsHardwareDecodeSupported(fourcc(b"hvc1")) != 0,
                VTIsHardwareDecodeSupported(vp9) != 0,
                VTIsHardwareDecodeSupported(av1) != 0,
            ];
            log::info!("decoded in hardware here: H.264 {}, HEVC {}, VP9 {}, AV1 {}", answers[0], answers[1], answers[2], answers[3]);
            answers
        });
        answers[family as usize]
    }
}

/// The Deck's VA-API decodes all four; a Beam Pro's MediaCodec does too. Asked of the hardware
/// only where it has been seen to matter.
#[cfg(not(target_os = "macos"))]
mod platform {
    use super::Family;

    pub fn decodes(_family: Family) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sound_and_unknown_codecs_are_never_turned_away() {
        assert!(decoded_in_hardware("mp4a.40.2"));
        assert!(decoded_in_hardware("opus"));
        assert!(decoded_in_hardware("vp8"));
    }

    #[test]
    fn the_software_list_names_what_is_turned_away() {
        let soft = software_only();
        for name in &soft {
            assert!(!decoded_in_hardware(name), "{name}");
        }
    }
}
