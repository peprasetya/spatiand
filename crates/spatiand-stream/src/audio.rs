//! An application's sound, on a stream of its own.
//!
//! Each application that makes a sound gets one unidirectional QUIC stream from the host,
//! opened the first time it plays after a session attaches. The stream starts with a small
//! header saying whose sound it is and in what shape, and after that it is nothing but samples:
//! signed 16-bit little-endian, interleaved, at [`RATE`].
//!
//! **An application's sound is raw; a voice is not.** Stereo at 48 kHz is 1.5 Mbit/s — a tenth
//! of what a moving picture costs — and leaving it raw keeps a codec off that path and adds no
//! delay of its own. Silence is not sent at all, so a quiet app costs nothing.
//!
//! The wearer's microphone goes the other way and is [`Coding::Opus`], because the uplink is
//! the scarce direction and a voice is the one thing a codec is unambiguously good at. Mono at
//! 48 kHz raw is 768 kbit/s; the same voice in Opus is around 24, and the encoder adds one
//! frame of delay rather than the hundreds of milliseconds a queue adds when a link cannot
//! keep up with raw. Nothing new is installed for it: the ffmpeg both ends already link
//! carries Opus, and they need not agree on a version — only on a codec.
//!
//! **A stream, not datagrams.** A lost piece of a picture costs a frame; a lost piece of sound
//! is a click, every time. Retransmission on a link with a 10 ms round trip is cheaper than any
//! concealment, and the receiving end keeps its own delay bounded by throwing away what has
//! fallen too far behind.

use serde::{Deserialize, Serialize};

/// Samples per second, per channel.
pub const RATE: u32 = 48_000;
/// Stereo: what an application is sent unless its catalogue entry asks for a wider sink.
/// Wider sound is a matter of the header, not the stream -- see [`channels_for`].
pub const CHANNELS: u16 = 2;
/// Bytes of one stereo frame (one sample for every channel).
pub const FRAME_BYTES: usize = CHANNELS as usize * 2;

/// The channel counts a stream may carry: stereo, 5.1, 7.1 and 7.1.4. The same layouts, in the
/// same order, as `spatiand_audio::stage::Layout`, which is what makes the sink at the other end
/// able to place them.
pub const WIDTHS: [u16; 4] = [2, 6, 8, 12];

/// Whether this end can play a stream of that many channels.
pub fn is_supported(channels: u16) -> bool {
    WIDTHS.contains(&channels)
}

/// Bytes of one frame of that many channels.
pub const fn frame_bytes(channels: u16) -> usize {
    channels as usize * 2
}

/// How many channels an application's sound is carried in, by what its catalogue entry asks.
///
/// **Stereo unless it asked for more.** That is the whole of "do not break the applications that
/// already work": a VR viewer mixes its own head-turned stereo and has to arrive as exactly that,
/// to be played as it is and not placed in the room, and an application that did not say
/// anything gets what it always got. Only an entry that names a surround layout is given a
/// surround sink, and the session then places it at its window like any local one.
///
/// Ambisonics is not a channel layout this can carry yet, and is stereo here.
pub fn channels_for(mode: crate::catalog::AudioMode) -> u16 {
    use crate::catalog::AudioMode::*;
    match mode {
        Auto | Stereo | Ambisonic => 2,
        Surround51 => 6,
        Surround71 => 8,
        Surround714 => 12,
    }
}

/// The names PipeWire gives the channels of a sink of that many, in wire order -- the order the
/// samples are interleaved in, which is `spatiand_audio::stage::Layout`'s. Space-separated, for
/// a sink's `audio.position`.
pub fn positions(channels: u16) -> &'static str {
    match channels {
        6 => "FL FR FC LFE RL RR",
        8 => "FL FR FC LFE RL RR SL SR",
        12 => "FL FR FC LFE RL RR SL SR TFL TFR TRL TRR",
        _ => "FL FR",
    }
}

/// The same, comma-separated, for `pw-cat --channel-map`.
pub fn channel_map(channels: u16) -> &'static str {
    match channels {
        6 => "FL,FR,FC,LFE,RL,RR",
        8 => "FL,FR,FC,LFE,RL,RR,SL,SR",
        12 => "FL,FR,FC,LFE,RL,RR,SL,SR,TFL,TFR,TRL,TRR",
        _ => "FL,FR",
    }
}

/// What every sound stream starts with, before its header, so a stream of anything else is
/// recognised and left alone.
pub const MAGIC: [u8; 4] = *b"SPau";

/// How the samples after the header are written.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Coding {
    /// Signed 16-bit little-endian, interleaved, and nothing else.
    #[default]
    Pcm,
    /// Opus packets, each behind its own length; see [`frame`] and [`Packets`].
    Opus,
}

/// Whose sound this is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioHeader {
    /// The catalogue id of the application.
    pub app: String,
    pub rate: u32,
    pub channels: u16,
    /// Absent from an older end, which only ever sent samples.
    #[serde(default)]
    pub coding: Coding,
}

/// The largest packet that will be believed.
///
/// An Opus frame of a voice is tens of bytes and never approaches this. It is here so that a
/// corrupt or hostile length cannot ask this end to set aside a gigabyte and wait for it.
pub const LARGEST_PACKET: usize = 8 << 10;

/// One packet on the wire: its length as a little-endian `u16`, then itself.
///
/// A stream of Opus needs its edges marked — a packet does not say how long it is, and the
/// decoder must be given exactly what the encoder produced. Two bytes of overhead on a frame
/// of about sixty is cheaper than any of the alternatives.
pub fn frame(packet: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(2 + packet.len());
    out.extend_from_slice(&(packet.len() as u16).to_le_bytes());
    out.extend_from_slice(packet);
    out
}

/// Whole packets, pulled out of a stream that arrives in whatever pieces it likes.
///
/// A QUIC stream is bytes, not messages: one read may hold three packets and half of a fourth,
/// or two bytes of a length. This keeps the tail until the rest of it turns up.
#[derive(Default)]
pub struct Packets {
    buffer: Vec<u8>,
}

impl Packets {
    /// Add what just arrived.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.buffer.extend_from_slice(bytes);
    }

    /// The next whole packet, if there is one. `Err` means the stream is not what it claims
    /// and should be abandoned rather than resynchronised — there is nothing to resynchronise
    /// to.
    pub fn next(&mut self) -> Result<Option<Vec<u8>>, &'static str> {
        if self.buffer.len() < 2 {
            return Ok(None);
        }
        let length = u16::from_le_bytes([self.buffer[0], self.buffer[1]]) as usize;
        if length > LARGEST_PACKET {
            return Err("a sound packet claimed to be far too long");
        }
        if self.buffer.len() < 2 + length {
            return Ok(None);
        }
        let packet = self.buffer[2..2 + length].to_vec();
        self.buffer.drain(..2 + length);
        Ok(Some(packet))
    }
}

impl AudioHeader {
    /// Magic, then the header's length as a little-endian u32, then the header.
    pub fn encode(&self) -> Vec<u8> {
        let body = crate::to_bytes(self).expect("a header always serialises");
        let mut out = Vec::with_capacity(8 + body.len());
        out.extend_from_slice(&MAGIC);
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&body);
        out
    }

    /// The header's length, from the eight bytes after the magic check — `None` if these are
    /// not the start of a sound stream.
    pub fn length(first: &[u8; 8]) -> Option<usize> {
        if first[..4] != MAGIC {
            return None;
        }
        let length = u32::from_le_bytes(first[4..8].try_into().ok()?) as usize;
        (length <= 4096).then_some(length)
    }

    pub fn decode(body: &[u8]) -> Option<AudioHeader> {
        crate::from_bytes(body).ok()
    }
}

/// Whether a stretch of samples is all silence, and so not worth sending.
pub fn is_silent(pcm: &[u8]) -> bool {
    pcm.iter().all(|b| *b == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_header_round_trips() {
        let header = AudioHeader {
            app: "chrome".into(),
            rate: RATE,
            channels: CHANNELS,
            coding: Coding::Opus,
        };
        let bytes = header.encode();
        let length = AudioHeader::length(bytes[..8].try_into().unwrap()).unwrap();
        assert_eq!(AudioHeader::decode(&bytes[8..8 + length]), Some(header));
    }

    #[test]
    fn a_surround_header_round_trips() {
        for channels in WIDTHS {
            let header = AudioHeader {
                app: "game".into(),
                rate: RATE,
                channels,
                coding: Coding::Pcm,
            };
            let bytes = header.encode();
            let length = AudioHeader::length(bytes[..8].try_into().unwrap()).unwrap();
            assert_eq!(AudioHeader::decode(&bytes[8..8 + length]), Some(header));
        }
    }

    #[test]
    fn only_a_catalogue_entry_that_asks_for_more_gets_more_than_stereo() {
        use crate::catalog::AudioMode::*;
        // The applications that already work -- anything that said nothing, and a viewer mixing
        // its own stereo -- are exactly as they were.
        for mode in [Auto, Stereo, Ambisonic] {
            assert_eq!(channels_for(mode), 2, "{mode:?}");
        }
        assert_eq!(channels_for(Surround51), 6);
        assert_eq!(channels_for(Surround71), 8);
        assert_eq!(channels_for(Surround714), 12);
        for mode in [Auto, Stereo, Surround51, Surround71, Surround714, Ambisonic] {
            assert!(is_supported(channels_for(mode)));
        }
        assert!(!is_supported(3) && !is_supported(0) && !is_supported(16));
    }

    #[test]
    fn the_channel_names_are_as_many_as_the_channels_and_in_wire_order() {
        for channels in WIDTHS {
            assert_eq!(positions(channels).split(' ').count(), channels as usize);
            assert_eq!(channel_map(channels).split(',').count(), channels as usize);
            assert_eq!(positions(channels).replace(' ', ","), channel_map(channels));
            assert_eq!(frame_bytes(channels), channels as usize * 2);
        }
        // Front pair first, then centre and LFE, then the rest, as `Layout` has them: getting
        // this wrong swaps somebody's surrounds, and nothing fails loudly.
        assert!(positions(8).starts_with("FL FR FC LFE RL RR SL SR"));
        assert!(positions(12).ends_with("TFL TFR TRL TRR"));
    }

    #[test]
    fn anything_else_is_not_a_sound_stream() {
        assert_eq!(AudioHeader::length(b"\x10\0\0\0abcd"), None);
    }

    #[test]
    fn packets_come_back_whole_however_they_arrive() {
        // The case that matters: the stream is cut in the middle of a length, and again in
        // the middle of a packet. Nothing may be lost or run together.
        let wire: Vec<u8> = [frame(b"one"), frame(b""), frame(b"a longer one")].concat();
        let mut packets = Packets::default();
        let mut out = Vec::new();
        for byte in wire {
            packets.feed(&[byte]);
            while let Some(packet) = packets.next().expect("the stream is well formed") {
                out.push(packet);
            }
        }
        assert_eq!(out, vec![b"one".to_vec(), Vec::new(), b"a longer one".to_vec()]);
    }

    #[test]
    fn several_packets_in_one_piece_all_come_out() {
        let mut packets = Packets::default();
        packets.feed(&[frame(b"aa"), frame(b"bbb")].concat());
        assert_eq!(packets.next(), Ok(Some(b"aa".to_vec())));
        assert_eq!(packets.next(), Ok(Some(b"bbb".to_vec())));
        assert_eq!(packets.next(), Ok(None));
    }

    #[test]
    fn a_length_nobody_could_mean_is_refused_rather_than_waited_for() {
        let mut packets = Packets::default();
        packets.feed(&(LARGEST_PACKET as u16 + 1).to_le_bytes());
        assert!(packets.next().is_err());
    }

    #[test]
    fn an_older_end_that_says_nothing_about_coding_means_samples() {
        // `coding` was added after the first sound streams existed. One that does not mention
        // it is the raw samples it always was, not an unreadable stream.
        let older = crate::to_bytes(&(String::from("chrome"), RATE, CHANNELS))
            .expect("this serialises");
        assert!(AudioHeader::decode(&older).is_none_or(|h| h.coding == Coding::Pcm));
    }
}
