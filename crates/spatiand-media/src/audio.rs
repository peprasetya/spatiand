//! Choosing a channel layout, and saying honestly what is being heard.
//!
//! Two separate jobs, both here because both turn on the same table.
//!
//! **The layout.** Spatiand gives every window a twelve-channel PipeWire sink and places
//! the channels as speakers in the room. It recognises six counts and, in its own words,
//! anything else "is refused and logged rather than guessed at - placing channels in the
//! wrong order is inaudible on a test tone and unmistakable on a film". FFmpeg will
//! cheerfully decode a 5.0 track to five channels and a 6.1 track to seven, neither of
//! which is on that list, and the symptom is a film with no sound at all. So the counts
//! are pinned rather than left to negotiation.
//!
//! **The honesty.** No decoder on Linux renders Atmos or DTS:X objects. FFmpeg detects
//! both and decodes neither: `ac3dec.c` labels E-AC-3 with JOC as
//! `AV_PROFILE_EAC3_DDP_ATMOS` and decodes its 5.1 bed, `mlpdec.c` limits TrueHD to three
//! substreams "as the 4th is used by Dolby Atmos for non-audio data", and `dca_exss.c`
//! parses DTS's extension substreams without any X object handling. What arrives is the
//! bed. Since Vibrefy serves only MP4 - where Atmos is almost always E-AC-3 with JOC -
//! that bed is usually 5.1. Saying so is better than a badge claiming Atmos.

/// The counts Spatiand's mixer recognises, and the layout each one means.
///
/// Wire order, from `docs/apps.md`:
/// `L R C LFE Ls Rs Lss Rss Ltf Rtf Ltr Rtr`.
pub const SPATIAND_LAYOUTS: [(u16, &str); 6] = [
    (1, "mono"),
    (2, "stereo"),
    (6, "5.1"),
    (8, "7.1"),
    (10, "5.1.4"),
    (12, "7.1.4"),
];

/// 7.1.4, written the only way mpv will take it.
///
/// mpv has no *name* for the height layouts - `--audio-channels=help` stops at the 7.1
/// variants - so asking for "7.1.4" is refused outright, and refusing one entry refuses the
/// whole property. Spelling the speakers out is what mpv accepts, and it has the useful
/// side effect of pinning the order rather than trusting two projects to agree on what a
/// name means.
///
/// The order is Spatiand's own, from `docs/apps.md`:
/// `L R C LFE Ls Rs Lss Rss Ltf Rtf Ltr Rtr`. Its 5.1 is `L R C LFE Ls Rs`, which matches
/// mpv's `5.1` (`fl-fr-fc-lfe-bl-br`), so `Ls`/`Rs` are the back pair and `Lss`/`Rss` the
/// sides.
pub const LAYOUT_7_1_4: &str = "fl-fr-fc-lfe-bl-br-sl-sr-tfl-tfr-tbl-tbr";

/// 5.1.4: the same without the side pair.
pub const LAYOUT_5_1_4: &str = "fl-fr-fc-lfe-bl-br-tfl-tfr-tbl-tbr";

/// What to give mpv's `--audio-channels`, widest first.
///
/// A list rather than `auto`: mpv picks the first that the output can carry and converts
/// to it, which is exactly the guarantee needed. `auto-safe` would pass a count straight
/// through, and a 5.0 film would arrive at Spatiand as five channels and be dropped.
pub const SPATIAND_CHANNELS: &str = concat!(
    "fl-fr-fc-lfe-bl-br-sl-sr-tfl-tfr-tbl-tbr,",
    "fl-fr-fc-lfe-bl-br-tfl-tfr-tbl-tbr,",
    "7.1,5.1,stereo,mono"
);

/// On an ordinary desktop nothing refuses an odd count, so nothing is forced: whatever the
/// file has goes to the sink, and PipeWire remixes for the actual output.
pub const DESKTOP_CHANNELS: &str = "auto-safe";

pub fn channel_preference(spatiand: bool) -> &'static str {
    if spatiand {
        SPATIAND_CHANNELS
    } else {
        DESKTOP_CHANNELS
    }
}

/// Passthrough sends the compressed stream to a receiver, which is the only way an Atmos
/// or DTS:X *object* mix is ever rendered on Linux.
///
/// Never under Spatiand: its spatialiser needs PCM channels to place, and a bitstream is
/// not something that can be put anywhere in a room. Offered on a desktop, where the Deck
/// may be docked to an AVR over HDMI.
pub fn passthrough_codecs(spatiand: bool, wanted: bool) -> Option<&'static str> {
    if spatiand || !wanted {
        None
    } else {
        Some("ac3,eac3,dts,dts-hd,truehd")
    }
}

pub fn layout_name(channels: u16) -> String {
    SPATIAND_LAYOUTS
        .iter()
        .find(|(count, _)| *count == channels)
        .map(|(_, name)| (*name).to_string())
        .unwrap_or_else(|| format!("{channels} channels"))
}

pub fn is_placeable(channels: u16) -> bool {
    SPATIAND_LAYOUTS.iter().any(|(count, _)| *count == channels)
}

/// The nearest count Spatiand will accept, never downward.
///
/// Rounding up pads with silence; rounding down would throw a channel away, and the one
/// most often thrown away is the centre - which is the dialogue.
pub fn nearest_placeable(channels: u16) -> u16 {
    SPATIAND_LAYOUTS
        .iter()
        .map(|(count, _)| *count)
        .find(|count| *count >= channels)
        .unwrap_or(12)
}

/// What to show in the audio menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioSummary {
    /// The headline: what is being heard, right now.
    pub heading: String,
    /// The caveat, when there is one worth making.
    pub note: Option<String>,
}

/// Reads mpv's `track-list` fields and says what they add up to.
///
/// `profile` is FFmpeg's codec profile as mpv passes it through - the exact strings are in
/// `libavcodec/profiles.c`, and the ones that matter here are "Dolby Digital Plus + Dolby
/// Atmos", "Dolby TrueHD + Dolby Atmos", "DTS-HD MA + DTS:X" and its IMAX variant.
pub fn summarise(codec: &str, profile: Option<&str>, channels: u16, passthrough: bool) -> AudioSummary {
    let layout = layout_name(channels);
    let profile = profile.unwrap_or_default();
    let object_format = if profile.contains("Atmos") {
        Some("Atmos")
    } else if profile.contains("DTS:X") {
        Some("DTS:X")
    } else {
        None
    };

    match (object_format, passthrough) {
        // The receiver is doing the rendering, so the objects really are being heard.
        (Some(format), true) => AudioSummary {
            heading: format!("{format}, sent to the receiver"),
            note: Some(format!(
                "{codec} is being passed through untouched; the objects are placed by the receiver, not here."
            )),
        },
        (Some(format), false) => AudioSummary {
            heading: format!("{format} - playing its {layout} bed"),
            note: Some(format!(
                "No decoder on Linux renders {format} objects; FFmpeg detects the extension and decodes the bed underneath it. Passthrough to a receiver is the only way to hear the objects.",
            )),
        },
        (None, true) => AudioSummary {
            heading: format!("{codec}, sent to the receiver"),
            note: None,
        },
        (None, false) => AudioSummary {
            heading: format!("{codec} {layout}"),
            note: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The table in `docs/apps.md`, kept honest.
    #[test]
    fn the_recognised_counts_are_the_ones_spatiand_lists() {
        for (count, name) in SPATIAND_LAYOUTS {
            assert!(is_placeable(count));
            assert_eq!(name, layout_name(count));
        }
        for odd in [0u16, 3, 4, 5, 7, 9, 11, 13] {
            assert!(!is_placeable(odd), "{odd} is not in the table");
        }
    }

    /// The silent-film failure. A 5.0 track is five channels and a 6.1 track is seven, and
    /// Spatiand drops both without a sound.
    #[test]
    fn an_unplaceable_count_is_rounded_up_to_one_that_works() {
        assert_eq!(6, nearest_placeable(5), "5.0 becomes 5.1");
        assert_eq!(8, nearest_placeable(7), "6.1 becomes 7.1");
        assert_eq!(6, nearest_placeable(3), "2.1 becomes 5.1");
        assert_eq!(6, nearest_placeable(4), "quad becomes 5.1");
        assert_eq!(10, nearest_placeable(9));
        assert_eq!(12, nearest_placeable(11));
    }

    /// Never downward: the channel a downward round would drop is the centre, and the
    /// centre is the dialogue.
    #[test]
    fn a_count_that_already_works_is_left_alone() {
        for (count, _) in SPATIAND_LAYOUTS {
            assert_eq!(count, nearest_placeable(count));
        }
    }

    #[test]
    fn more_channels_than_the_room_has_are_capped_rather_than_refused() {
        assert_eq!(12, nearest_placeable(16));
    }

    /// `auto-safe` passes a count straight through, which is right on a desktop and wrong
    /// in a room that refuses counts it does not recognise.
    #[test]
    fn the_room_gets_an_explicit_list_and_a_desktop_does_not() {
        assert_eq!(SPATIAND_CHANNELS, channel_preference(true));
        assert_eq!("auto-safe", channel_preference(false));
    }

    /// Every entry has to be a count Spatiand recognises, and they have to be in
    /// descending order - mpv takes the first the output can carry.
    ///
    /// This is the test that would have caught the layouts being refused. It cannot check
    /// that mpv *parses* them - that needs mpv - but it does check the thing the names
    /// were standing in for, which is how many speakers each one has.
    #[test]
    fn every_layout_offered_to_the_room_is_one_the_room_accepts() {
        let counts: Vec<u16> = SPATIAND_CHANNELS.split(',').map(speaker_count).collect();
        assert_eq!(vec![12, 10, 8, 6, 2, 1], counts);
        for count in &counts {
            assert!(is_placeable(*count), "{count} is not in Spatiand's table");
        }
    }

    /// The height layouts are spelled out because mpv has no name for them, so their
    /// contents are worth asserting rather than assuming.
    #[test]
    fn the_height_layouts_carry_the_speakers_their_names_imply() {
        assert_eq!(12, speaker_count(LAYOUT_7_1_4));
        assert_eq!(10, speaker_count(LAYOUT_5_1_4));
        // Four height channels in each: top front pair and top back pair.
        for layout in [LAYOUT_7_1_4, LAYOUT_5_1_4] {
            let tops = layout.split('-').filter(|s| s.starts_with('t')).count();
            assert_eq!(4, tops, "{layout}");
            assert!(layout.starts_with("fl-fr-fc-lfe"), "the front stage comes first");
        }
        // 7.1.4 is 5.1.4 plus the side pair, which is the whole difference.
        assert!(LAYOUT_7_1_4.contains("-sl-sr-"));
        assert!(!LAYOUT_5_1_4.contains("-sl-sr-"));
    }

    /// A named layout counts its channels; a spelled-out one counts its speakers.
    fn speaker_count(layout: &str) -> u16 {
        match layout {
            "mono" => 1,
            "stereo" => 2,
            "5.1" => 6,
            "7.1" => 8,
            other => other.split('-').count() as u16,
        }
    }

    /// A bitstream cannot be placed in a room, so the spatialiser must never be handed one.
    #[test]
    fn passthrough_is_refused_in_the_room_however_it_is_asked_for() {
        assert_eq!(None, passthrough_codecs(true, true));
        assert_eq!(None, passthrough_codecs(true, false));
        assert_eq!(None, passthrough_codecs(false, false));
        assert!(passthrough_codecs(false, true).is_some());
    }

    /// The whole point of the honesty: an Atmos file on this server is E-AC-3 with JOC,
    /// and what comes out of the decoder is its 5.1 bed.
    #[test]
    fn an_atmos_track_says_which_part_of_it_is_being_heard() {
        let summary = summarise("eac3", Some("Dolby Digital Plus + Dolby Atmos"), 6, false);
        assert_eq!("Atmos - playing its 5.1 bed", summary.heading);
        assert!(summary.note.unwrap().contains("decodes the bed"));
    }

    #[test]
    fn a_truehd_atmos_track_names_its_wider_bed() {
        let summary = summarise("truehd", Some("Dolby TrueHD + Dolby Atmos"), 8, false);
        assert_eq!("Atmos - playing its 7.1 bed", summary.heading);
    }

    /// Both DTS:X profile strings, as `libavcodec/profiles.c` spells them.
    #[test]
    fn dts_x_is_recognised_including_the_imax_variant() {
        for profile in ["DTS-HD MA + DTS:X", "DTS-HD MA + DTS:X IMAX"] {
            let summary = summarise("dts", Some(profile), 8, false);
            assert_eq!("DTS:X - playing its 7.1 bed", summary.heading, "{profile}");
        }
    }

    /// Passing through means the receiver really is rendering the objects, and claiming
    /// only a bed would then be the dishonest answer.
    #[test]
    fn passthrough_does_not_apologise_for_a_bed_it_is_not_playing() {
        let summary = summarise("truehd", Some("Dolby TrueHD + Dolby Atmos"), 8, true);
        assert_eq!("Atmos, sent to the receiver", summary.heading);
        assert!(summary.note.unwrap().contains("placed by the receiver"));
    }

    #[test]
    fn an_ordinary_track_gets_no_caveat() {
        let summary = summarise("eac3", Some("DTS-HD MA"), 6, false);
        assert_eq!("eac3 5.1", summary.heading);
        assert_eq!(None, summary.note);

        let stereo = summarise("aac", None, 2, false);
        assert_eq!("aac stereo", stereo.heading);
        assert_eq!(None, stereo.note);
    }
}
