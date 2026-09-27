//! A Matroska file, written as it is recorded: one H.264 picture track and any number of
//! uncompressed sound tracks.
//!
//! The Deck writes its recordings through ffmpeg, which Android has not got, and Android's own
//! muxer writes MP4, which takes sound only as AAC -- and this phone's AAC encoder stops at six
//! channels, where the surround track is twelve. Matroska takes PCM of any width, so the Beam
//! Pro's recordings are the Deck's: the same container, the same tracks.
//!
//! Only what that needs: the EBML header, one segment with its info and tracks, and clusters of
//! simple blocks, each cluster built in memory and written whole. The segment's size and the
//! duration are written last, over the places kept for them, so players can seek.

use std::fs::File;
use std::io::{Seek, SeekFrom, Write};

/// A track, as it is described in the file.
pub enum Track {
    /// H.264, `private` its `avcC` record (see [`avc_config`]); both eyes side by side.
    Video { width: u32, height: u32, private: Vec<u8> },
    /// Little-endian 24-bit PCM.
    Audio { title: String, channels: usize, rate: u32, default: bool },
}

/// How long a cluster runs before the next begins, in milliseconds.
const CLUSTER_MS: i64 = 1000;

pub struct Writer {
    file: File,
    /// Where the segment's contents begin, and where its size and the duration are to be
    /// written when the file is finished.
    segment_data: u64,
    segment_size_at: u64,
    duration_at: u64,
    cluster: Vec<u8>,
    cluster_ms: Option<i64>,
    last_ms: i64,
}

impl Writer {
    pub fn create(mut file: File, title: &str, tracks: &[Track]) -> std::io::Result<Writer> {
        let mut header = Vec::new();
        element(&mut header, 0x1A45DFA3, &{
            let mut e = Vec::new();
            uint(&mut e, 0x4286, 1);
            uint(&mut e, 0x42F7, 1);
            uint(&mut e, 0x42F2, 4);
            uint(&mut e, 0x42F3, 8);
            string(&mut e, 0x4282, "matroska");
            uint(&mut e, 0x4287, 4);
            uint(&mut e, 0x4285, 2);
            e
        });
        // The segment, its size to follow.
        id(&mut header, 0x18538067);
        let segment_size_at = header.len() as u64;
        header.extend_from_slice(&[0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]);
        let segment_data = header.len() as u64;

        let mut info = Vec::new();
        uint(&mut info, 0x2AD7B1, 1_000_000); // milliseconds
        string(&mut info, 0x4D80, "Spatiand");
        string(&mut info, 0x5741, "Spatiand");
        string(&mut info, 0x7BA9, title);
        // Duration: a float written over when the file is finished.
        id(&mut info, 0x4489);
        size(&mut info, 8);
        let duration_in_info = info.len();
        info.extend_from_slice(&0f64.to_be_bytes());
        id(&mut header, 0x1549A966);
        size(&mut header, info.len() as u64);
        let duration_at = header.len() as u64 + duration_in_info as u64;
        header.extend_from_slice(&info);

        let mut entries = Vec::new();
        for (i, track) in tracks.iter().enumerate() {
            let mut t = Vec::new();
            uint(&mut t, 0xD7, i as u64 + 1);
            uint(&mut t, 0x73C5, i as u64 + 1);
            uint(&mut t, 0x9C, 0);
            string(&mut t, 0x22B59C, "und");
            match track {
                Track::Video { width, height, private } => {
                    uint(&mut t, 0x83, 1);
                    uint(&mut t, 0x88, 1);
                    string(&mut t, 0x86, "V_MPEG4/ISO/AVC");
                    element(&mut t, 0x63A2, private);
                    element(&mut t, 0xE0, &{
                        let mut v = Vec::new();
                        uint(&mut v, 0xB0, *width as u64);
                        uint(&mut v, 0xBA, *height as u64);
                        // Side by side, the left eye first.
                        uint(&mut v, 0x53B8, 1);
                        v
                    });
                }
                Track::Audio { title, channels, rate, default } => {
                    uint(&mut t, 0x83, 2);
                    uint(&mut t, 0x88, *default as u64);
                    string(&mut t, 0x536E, title);
                    string(&mut t, 0x86, "A_PCM/INT/LIT");
                    element(&mut t, 0xE1, &{
                        let mut a = Vec::new();
                        float(&mut a, 0xB5, *rate as f64);
                        uint(&mut a, 0x9F, *channels as u64);
                        uint(&mut a, 0x6264, 24);
                        a
                    });
                }
            }
            element(&mut entries, 0xAE, &t);
        }
        element(&mut header, 0x1654AE6B, &entries);
        file.write_all(&header)?;
        Ok(Writer {
            file,
            segment_data,
            segment_size_at,
            duration_at,
            cluster: Vec::new(),
            cluster_ms: None,
            last_ms: 0,
        })
    }

    /// One block: `track` counted from 1 as given to [`Writer::create`], at `ms` from the start.
    fn block(&mut self, track: usize, ms: i64, keyframe: bool, data: &[u8]) -> std::io::Result<()> {
        let start = match self.cluster_ms {
            Some(start) if (ms - start).abs() < CLUSTER_MS && ms - start < i16::MAX as i64 => start,
            _ => {
                self.flush_cluster()?;
                uint(&mut self.cluster, 0xE7, ms.max(0) as u64);
                self.cluster_ms = Some(ms.max(0));
                ms.max(0)
            }
        };
        let relative = (ms - start).clamp(i16::MIN as i64, i16::MAX as i64) as i16;
        let mut body = Vec::with_capacity(data.len() + 4);
        body.push(0x80 | track as u8);
        body.extend_from_slice(&relative.to_be_bytes());
        body.push(if keyframe { 0x80 } else { 0 });
        body.extend_from_slice(data);
        element(&mut self.cluster, 0xA3, &body);
        self.last_ms = self.last_ms.max(ms);
        Ok(())
    }

    /// A coded picture, as the encoder gave it (Annex B), at `ms`.
    pub fn video(&mut self, track: usize, ms: i64, keyframe: bool, annex_b: &[u8]) -> std::io::Result<()> {
        let mut sized = Vec::with_capacity(annex_b.len() + 16);
        for nal in nal_units(annex_b) {
            sized.extend_from_slice(&(nal.len() as u32).to_be_bytes());
            sized.extend_from_slice(nal);
        }
        self.block(track, ms, keyframe, &sized)
    }

    /// Sound, interleaved floats, starting at `ms`.
    pub fn audio(&mut self, track: usize, ms: i64, samples: &[f32]) -> std::io::Result<()> {
        let mut bytes = Vec::with_capacity(samples.len() * 3);
        for s in samples {
            let v = (s.clamp(-1.0, 1.0) * 8_388_607.0) as i32;
            bytes.extend_from_slice(&v.to_le_bytes()[..3]);
        }
        self.block(track, ms, true, &bytes)
    }

    fn flush_cluster(&mut self) -> std::io::Result<()> {
        if self.cluster_ms.is_none() {
            return Ok(());
        }
        let mut out = Vec::with_capacity(self.cluster.len() + 12);
        element(&mut out, 0x1F43B675, &self.cluster);
        self.file.write_all(&out)?;
        self.cluster.clear();
        self.cluster_ms = None;
        Ok(())
    }

    /// Write the last cluster, the segment's size and the duration, `ms` long.
    pub fn finish(mut self, ms: i64) -> std::io::Result<()> {
        self.flush_cluster()?;
        let end = self.file.stream_position()?;
        let segment = end - self.segment_data;
        self.file.seek(SeekFrom::Start(self.segment_size_at))?;
        let mut sized = (segment | (1u64 << 56)).to_be_bytes();
        sized[0] = 0x01;
        self.file.write_all(&sized)?;
        self.file.seek(SeekFrom::Start(self.duration_at))?;
        self.file.write_all(&(ms.max(self.last_ms) as f64).to_be_bytes())?;
        self.file.flush()
    }
}

/// The `avcC` record Matroska keeps for H.264, from the stream's SPS and PPS as the encoder
/// gave them (Annex B, in its codec-config packet).
pub fn avc_config(annex_b: &[u8]) -> Option<Vec<u8>> {
    let nals: Vec<&[u8]> = nal_units(annex_b).collect();
    let sps = nals.iter().find(|n| n.first().map(|b| b & 0x1F) == Some(7))?;
    let pps = nals.iter().find(|n| n.first().map(|b| b & 0x1F) == Some(8))?;
    if sps.len() < 4 {
        return None;
    }
    let mut out = vec![1, sps[1], sps[2], sps[3], 0xFF, 0xE1];
    out.extend_from_slice(&(sps.len() as u16).to_be_bytes());
    out.extend_from_slice(sps);
    out.push(1);
    out.extend_from_slice(&(pps.len() as u16).to_be_bytes());
    out.extend_from_slice(pps);
    Some(out)
}

/// The NAL units of an Annex B stream, without their start codes.
fn nal_units(data: &[u8]) -> impl Iterator<Item = &[u8]> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 <= data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            starts.push(i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }
    let ends: Vec<usize> = starts
        .iter()
        .skip(1)
        .map(|&s| {
            // Back over the start code, and the zero of a four-byte one.
            let mut e = s - 3;
            if e > 0 && data[e - 1] == 0 {
                e -= 1;
            }
            e
        })
        .chain(std::iter::once(data.len()))
        .collect();
    starts
        .into_iter()
        .zip(ends)
        .filter(|(s, e)| e > s)
        .map(move |(s, e)| &data[s..e])
}

fn id(out: &mut Vec<u8>, id: u32) {
    let bytes = id.to_be_bytes();
    let skip = bytes.iter().position(|b| *b != 0).unwrap_or(3);
    out.extend_from_slice(&bytes[skip..]);
}

fn size(out: &mut Vec<u8>, n: u64) {
    // The shortest width that holds it, the all-ones value of each being reserved.
    for width in 1..=8u32 {
        if n < (1u64 << (7 * width)) - 1 {
            let marked = n | (1u64 << (7 * width));
            out.extend_from_slice(&marked.to_be_bytes()[8 - width as usize..]);
            return;
        }
    }
    out.extend_from_slice(&[0x01, 0, 0, 0, 0, 0, 0, 0]);
}

fn element(out: &mut Vec<u8>, element: u32, body: &[u8]) {
    id(out, element);
    size(out, body.len() as u64);
    out.extend_from_slice(body);
}

fn uint(out: &mut Vec<u8>, element_id: u32, value: u64) {
    let bytes = value.to_be_bytes();
    let skip = bytes.iter().position(|b| *b != 0).unwrap_or(7);
    element(out, element_id, &bytes[skip..]);
}

fn float(out: &mut Vec<u8>, element_id: u32, value: f64) {
    element(out, element_id, &value.to_be_bytes());
}

fn string(out: &mut Vec<u8>, element_id: u32, value: &str) {
    element(out, element_id, value.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_use_the_shortest_width() {
        let mut v = Vec::new();
        size(&mut v, 5);
        assert_eq!(v, [0x85]);
        v.clear();
        size(&mut v, 200);
        assert_eq!(v, [0x40, 200]);
        v.clear();
        size(&mut v, 127);
        assert_eq!(v, [0x40, 127], "0x7F alone would mean unknown");
    }

    #[test]
    fn nal_units_are_split_at_three_and_four_byte_start_codes() {
        let stream = [0, 0, 0, 1, 0x67, 0x64, 0, 0x33, 0, 0, 1, 0x68, 3, 0, 0, 0, 1, 0x65, 9];
        let nals: Vec<&[u8]> = nal_units(&stream).collect();
        assert_eq!(nals, vec![&[0x67, 0x64, 0, 0x33][..], &[0x68, 3][..], &[0x65, 9][..]]);
    }

    #[test]
    fn the_avc_record_carries_the_profile_and_both_parameter_sets() {
        let config = [0, 0, 0, 1, 0x67, 0x64, 0, 0x33, 0, 0, 0, 1, 0x68, 3];
        let record = avc_config(&config).unwrap();
        assert_eq!(
            record,
            vec![1, 0x64, 0, 0x33, 0xFF, 0xE1, 0, 4, 0x67, 0x64, 0, 0x33, 1, 0, 2, 0x68, 3]
        );
    }
}
