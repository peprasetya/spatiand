//! The picture a film or a song carries of itself -- its cover -- which a library shows it by.
//!
//! Read by Spatiand's own FFmpeg, never by the tools that write covers: every kind comes out of
//! FFmpeg the same way, as an *attached picture* -- a video stream marked
//! `AV_DISPOSITION_ATTACHED_PIC` whose one picture is its `attached_pic` packet, read with the
//! file's header. As FFmpeg 9.1 reads each kind (its demuxers' sources looked at, and each kind
//! tried with a file made for it: this module's tests):
//!
//! - **Matroska** (MKV, MKA, WebM), as mkvtoolnix writes covers: files attached to it, named as
//!   Matroska's conventions name them -- `cover.jpg` or `.png` (portrait or square, 600 pixels on
//!   its short side), `small_cover` (120), and `cover_land`, `small_cover_land` the landscape
//!   ones. An attached file of type `image/jpeg`, `image/png`, `image/gif` or `image/tiff` is an
//!   attached picture; of any other type -- `image/webp`, the wrong but often written
//!   `image/jpg`, `application/octet-stream` -- it is an *attachment* stream
//!   (`AVMEDIA_TYPE_ATTACHMENT`, codec none) with the file's bytes as its extradata. Either way
//!   the stream's tags are the file's name and type (`filename`, `mimetype`). Both are read here:
//!   an attachment is taken for a picture by what its bytes begin with, and its type or name
//!   saying it is one.
//! - **MP4, M4V, M4A, MOV**: the pictures of iTunes' `covr` atom, as AtomicParsley, MP4Box and
//!   Subler write them, each an attached picture (one whose data is marked neither JPEG, PNG nor
//!   BMP -- type 0 -- FFmpeg passes over); QuickTime's `com.apple.quicktime.artwork` the same.
//! - **MP3** (and AIFF and WAV with an ID3 chunk): ID3v2's `APIC` frames, each an attached
//!   picture whose `comment` is the picture's kind as ID3 names it ("Cover (front)").
//! - **FLAC**: its `PICTURE` blocks, the same.
//! - **Ogg** (Vorbis, Opus, Speex, FLAC, Theora): `METADATA_BLOCK_PICTURE` comments -- FLAC's
//!   picture block in base64 -- the same.
//! - WMA's `WM/Picture` (a `comment` the same) and APE tags' `Cover Art (Front)` come the same
//!   way; not tried.
//!
//! Where there are several, the cover is ([`rank`]): Matroska's `cover`, or a picture said to be
//! the front cover, or one that says nothing of what it is (MP4's covr is the cover); then
//! `cover_land`, `small_cover` and `small_cover_land` -- the portrait cover first, the owner's
//! word for a tile, and the large before the small, which would be blurred; then one called a
//! poster or the like, or of ID3's kind "Other"; then any other picture the file carries -- a back
//! cover, an artist, a logo -- before none at all. Of two alike, the first in the file.
//!
//! Nothing is read but what opening the file reads: a cover is in the file's header -- or, for an
//! MP4 not made for streaming, in its index at the end, which FFmpeg seeks to -- so a file on a
//! server is not fetched whole for it. [`tile`] goes on to a frame, as [`still::grab`] takes one,
//! only where there is no cover.
//!
//! [`still::grab`]: crate::still::grab

use std::ffi::{c_int, CStr, CString};
use std::path::Path;
use std::ptr;

use rsmpeg::ffi;

use crate::still::{self, Still};

/// The cover `path` carries -- a file, or an address FFmpeg reads -- no wider than `width`.
/// `None` if it carries none, or none that decodes.
pub fn find(path: &str, width: u32) -> Option<Still> {
    still::opened_with(CString::new(path).ok()?, options(path), |format| unsafe { from_open(format, width) })
}

/// The same, of a file on this machine by the system's own name for it.
pub fn find_file(path: &Path, width: u32) -> Option<Still> {
    still::opened_with(still::file_name(path)?, &[], |format| unsafe { from_open(format, width) })
}

/// **What a library shows for a film or a song**: the cover it carries, and where it carries
/// none, a frame of it a little way in (as [`still::grab`](crate::still::grab) takes one) -- the
/// file opened once for both. `None` for a file with neither: a song without a cover, or anything
/// FFmpeg cannot read. SpatiShelf's tiles and SpatiReel's are this, on every platform.
pub fn tile(path: &str, width: u32) -> Option<Still> {
    still::opened_with(CString::new(path).ok()?, options(path), |format| unsafe { tile_of(format, width) })
}

/// The same, of a file on this machine by the system's own name for it.
pub fn tile_file(path: &Path, width: u32) -> Option<Still> {
    still::opened_with(still::file_name(path)?, &[], |format| unsafe { tile_of(format, width) })
}

/// What FFmpeg's own `http:` is told for a file on a server (SpatiShelf's remotes, through the
/// storage service): to ask for no more than a megabyte at a time while it reads the header --
/// a request left open asks the server for all the rest of the file, and it goes on sending
/// until the connection is closed -- and to give up on a server silent for 20 seconds, rather
/// than keep a library's pictures waiting for ever. Measured (this module's test, a server that
/// counts what it sends): the cover of a 46 MB Matroska film cost 131 KB, of a 69 MB MP4 whose
/// index is at its end about 200 KB, a frame of a 46 MB film with no cover about half a
/// megabyte; as much as FFmpeg's own way, each request bounded. The player's `recv_buffer_size`
/// of a megabyte is not told: a socket's own starts smaller, and with it these cost four times as
/// much.
const NETWORK: &[(&str, &str)] = &[("initial_request_size", "1048576"), ("rw_timeout", "20000000")];

fn options(path: &str) -> &'static [(&'static str, &'static str)] {
    if path.starts_with("http://") || path.starts_with("https://") {
        crate::trusted_certificates();
        NETWORK
    } else {
        &[]
    }
}

unsafe fn tile_of(format: *mut ffi::AVFormatContext, width: u32) -> Option<Still> {
    if let Some(cover) = from_open(format, width) {
        return Some(cover);
    }
    // No pictures of its own and a container that names all its streams in its header: nothing
    // to take a frame of -- a song with no cover -- and nothing more to read to know it.
    let streams = std::slice::from_raw_parts((*format).streams, (*format).nb_streams as usize);
    let named_all = (*format).ctx_flags & ffi::AVFMTCTX_NOHEADER as c_int == 0;
    let filmed = streams.iter().any(|&stream| (*(*stream).codecpar).codec_type == ffi::AVMEDIA_TYPE_VIDEO && (*stream).disposition & ffi::AV_DISPOSITION_ATTACHED_PIC as c_int == 0);
    if named_all && !filmed {
        return None;
    }
    still::frame_of(format, width)
}

/// The cover of an open file: each picture it carries, nearest to being the cover first, until
/// one decodes.
pub(crate) unsafe fn from_open(format: *mut ffi::AVFormatContext, width: u32) -> Option<Still> {
    let mut carried = carried(format);
    carried.sort_by_key(|picture| (picture.rank, picture.stream));
    carried.iter().find_map(|picture| decoded(format, picture, width))
}

/// Of the attached pictures of an open file -- the streams a player can play -- the one that is
/// its cover: what a song plays with, where it has several (a front and a back cover, or
/// Matroska's four).
pub(crate) unsafe fn chosen(format: *mut ffi::AVFormatContext) -> Option<usize> {
    let mut carried: Vec<Carried> = carried(format).into_iter().filter(|picture| !picture.attachment).collect();
    carried.sort_by_key(|picture| (picture.rank, picture.stream));
    carried.iter().find(|picture| !ffi::avcodec_find_decoder(picture.codec).is_null()).map(|picture| picture.stream)
}

/// A picture a file carries.
struct Carried {
    /// Which stream it is.
    stream: usize,
    /// How near it is to being the cover ([`rank`]).
    rank: u8,
    /// Its bytes are an attachment's extradata, not an attached picture's packet.
    attachment: bool,
    /// What decodes it: what its bytes begin with says, or else what the file said it is.
    codec: ffi::AVCodecID,
}

unsafe fn carried(format: *mut ffi::AVFormatContext) -> Vec<Carried> {
    let mut carried = Vec::new();
    let streams = std::slice::from_raw_parts((*format).streams, (*format).nb_streams as usize);
    for (index, &stream) in streams.iter().enumerate() {
        let codecpar = (*stream).codecpar;
        let disposition = (*stream).disposition;
        let metadata = (*stream).metadata;
        let name = tag(metadata, c"filename");
        let (attachment, bytes) = if disposition & ffi::AV_DISPOSITION_ATTACHED_PIC as c_int != 0 {
            // A stream of pictures marked with chapters (MP4's timed thumbnails) has no one
            // picture of its own.
            if disposition & ffi::AV_DISPOSITION_TIMED_THUMBNAILS as c_int != 0 {
                continue;
            }
            let packet = &(*stream).attached_pic;
            (false, bytes(packet.data, packet.size))
        } else if (*codecpar).codec_type == ffi::AVMEDIA_TYPE_ATTACHMENT {
            // An attached file is a picture only where it says it is one, by its type or its
            // name; a font's first bytes are not mistaken for a picture's.
            let kind = tag(metadata, c"mimetype").unwrap_or_default().to_ascii_lowercase();
            if !(kind.starts_with("image/") || name.as_deref().is_some_and(pictured_name)) {
                continue;
            }
            (true, bytes((*codecpar).extradata, (*codecpar).extradata_size))
        } else {
            continue;
        };
        let Some(bytes) = bytes else { continue };
        let codec = match sniffed(bytes) {
            Some(codec) => codec,
            None if attachment => continue,
            None => (*codecpar).codec_id,
        };
        // An APE tag's picture is named by its key, "Cover Art (Front)", and its value is the
        // file's name.
        let ape = ffi::av_dict_get(metadata, c"Cover Art (".as_ptr(), ptr::null(), ffi::AV_DICT_IGNORE_SUFFIX as c_int);
        let ape = (!ape.is_null()).then(|| text((*ape).key)).flatten();
        let kind = tag(metadata, c"comment").or(ape);
        carried.push(Carried { stream: index, rank: rank(name.as_deref(), kind.as_deref()), attachment, codec });
    }
    carried
}

/// A picture's bytes, decoded and made a [`Still`] no wider than `width`.
unsafe fn decoded(format: *mut ffi::AVFormatContext, picture: &Carried, width: u32) -> Option<Still> {
    let stream = *(*format).streams.add(picture.stream);
    let decoder = ffi::avcodec_find_decoder(picture.codec);
    if decoder.is_null() {
        return None;
    }
    let mut packet = ffi::av_packet_alloc();
    if packet.is_null() {
        return None;
    }
    let made = if picture.attachment {
        let codecpar = (*stream).codecpar;
        let size = (*codecpar).extradata_size;
        let made = ffi::av_new_packet(packet, size);
        if made >= 0 {
            ptr::copy_nonoverlapping((*codecpar).extradata, (*packet).data, size as usize);
        }
        made
    } else {
        ffi::av_packet_ref(packet, &(*stream).attached_pic)
    };
    let mut still = None;
    if made >= 0 {
        let mut context = ffi::avcodec_alloc_context3(decoder);
        if !context.is_null() {
            (*context).thread_count = 1;
            if ffi::avcodec_open2(context, decoder, ptr::null_mut()) >= 0 {
                let mut frame = ffi::av_frame_alloc();
                // A picture is one packet: told there is no more, the decoder gives it up.
                if !frame.is_null() && ffi::avcodec_send_packet(context, packet) >= 0 {
                    ffi::avcodec_send_packet(context, ptr::null());
                    if ffi::avcodec_receive_frame(context, frame) >= 0 {
                        still = still::still_of(frame, width);
                    }
                }
                ffi::av_frame_free(&mut frame);
            }
            ffi::avcodec_free_context(&mut context);
        }
    }
    ffi::av_packet_free(&mut packet);
    still
}

/// How near a picture is to being the cover, by what the file says of it: 0 the cover itself,
/// higher further from it. `name` is the name of an attached file (Matroska's); `kind` what the
/// picture is said to be (ID3's, FLAC's, Vorbis comments' and WMA's picture kinds, an APE tag's
/// key) -- neither, for MP4's covr, which is the cover.
fn rank(name: Option<&str>, kind: Option<&str>) -> u8 {
    if let Some(kind) = kind {
        let kind = kind.trim().to_ascii_lowercase();
        return match kind.as_str() {
            "" | "cover (front)" | "cover art (front)" => 0,
            "other" => 4,
            icon if icon.contains("file icon") => 6,
            _ => 5,
        };
    }
    let Some(name) = name else { return 0 };
    let stem = name.rsplit_once('.').map_or(name, |(stem, _)| stem).trim().to_ascii_lowercase();
    match stem.as_str() {
        "cover" => 0,
        "cover_land" => 1,
        "small_cover" => 2,
        "small_cover_land" => 3,
        named if ["cover", "poster", "folder", "front"].iter().any(|word| named.contains(word)) => 4,
        _ => 5,
    }
}

/// Whether an attached file's name is a picture's, by its ending.
fn pictured_name(name: &str) -> bool {
    let ending = name.rsplit_once('.').map_or("", |(_, ending)| ending).to_ascii_lowercase();
    ["jpg", "jpeg", "jpe", "png", "webp", "gif", "bmp", "tif", "tiff"].contains(&ending.as_str())
}

/// What decodes a picture, by what its bytes begin with.
fn sniffed(bytes: &[u8]) -> Option<ffi::AVCodecID> {
    Some(match bytes {
        [0xff, 0xd8, 0xff, ..] => ffi::AV_CODEC_ID_MJPEG,
        [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, ..] => ffi::AV_CODEC_ID_PNG,
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => ffi::AV_CODEC_ID_WEBP,
        [b'G', b'I', b'F', b'8', b'7' | b'9', b'a', ..] => ffi::AV_CODEC_ID_GIF,
        [b'I', b'I', 0x2a, 0x00, ..] | [b'M', b'M', 0x00, 0x2a, ..] => ffi::AV_CODEC_ID_TIFF,
        [b'B', b'M', ..] => ffi::AV_CODEC_ID_BMP,
        _ => return None,
    })
}

unsafe fn bytes<'a>(data: *const u8, size: c_int) -> Option<&'a [u8]> {
    (!data.is_null() && size > 0).then(|| std::slice::from_raw_parts(data, size as usize))
}

unsafe fn text(value: *const std::ffi::c_char) -> Option<String> {
    (!value.is_null()).then(|| CStr::from_ptr(value).to_string_lossy().into_owned())
}

unsafe fn tag(dictionary: *mut ffi::AVDictionary, key: &CStr) -> Option<String> {
    let entry = ffi::av_dict_get(dictionary, key.as_ptr(), ptr::null(), 0);
    if entry.is_null() {
        None
    } else {
        text((*entry).value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    // The colours each picture is made of: which one was chosen is told by its colour.
    const RED: [u8; 3] = [220, 40, 40];
    const GREEN: [u8; 3] = [40, 190, 70];
    const BLUE: [u8; 3] = [40, 70, 220];
    const YELLOW: [u8; 3] = [230, 210, 40];
    /// The film's own pictures (`FILMED`).
    const FILM: [u8; 3] = [150, 100, 230];

    /// Three seconds of the film's own colour, 320 by 180, as ffmpeg's arguments.
    const FILMED: [&str; 4] = ["-f", "lavfi", "-i", "color=c=0x9664e6:s=320x180:r=10:d=3"];
    /// Three seconds of a tone.
    const SOUNDED: [&str; 4] = ["-f", "lavfi", "-i", "sine=frequency=440:duration=3"];

    /// Test media made by the system's ffmpeg program (deepmagpie's /usr/bin/ffmpeg, or
    /// SPATIAND_TEST_FFMPEG) -- not Spatiand's FFmpeg, which only reads them -- in a folder of
    /// the test's own, taken away after.
    struct Made {
        ffmpeg: PathBuf,
        dir: PathBuf,
    }

    impl Made {
        /// `None`, said, on a machine with no ffmpeg program to make them: nothing to try.
        fn new(test: &str) -> Option<Made> {
            let ffmpeg = std::env::var_os("SPATIAND_TEST_FFMPEG")
                .map(PathBuf::from)
                .or_else(|| ["/usr/bin/ffmpeg", "/opt/homebrew/bin/ffmpeg", "/usr/local/bin/ffmpeg"].iter().map(PathBuf::from).find(|p| p.exists()));
            let Some(ffmpeg) = ffmpeg else {
                eprintln!("{test}: no ffmpeg program to make the test media with (SPATIAND_TEST_FFMPEG): not tried");
                return None;
            };
            let dir = std::env::temp_dir().join(format!("spatiand-cover-{test}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Some(Made { ffmpeg, dir })
        }

        fn path(&self, name: &str) -> PathBuf {
            self.dir.join(name)
        }

        /// ffmpeg, in the test's folder, with these arguments.
        fn run(&self, args: &[&str]) {
            let out = std::process::Command::new(&self.ffmpeg).args(["-nostdin", "-y", "-hide_banner", "-loglevel", "error"]).args(args).current_dir(&self.dir).output().unwrap();
            assert!(out.status.success(), "ffmpeg {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        }

        /// A picture of one colour, `w` by `h`, in the format its name's ending says; its name.
        fn picture(&self, name: &str, colour: [u8; 3], w: u32, h: u32) -> String {
            let source = format!("color=c=0x{:02x}{:02x}{:02x}:s={w}x{h}", colour[0], colour[1], colour[2]);
            self.run(&["-f", "lavfi", "-i", &source, "-frames:v", "1", "-update", "1", "-q:v", "2", name]);
            name.to_string()
        }

        /// The film (`FILMED`) with these files attached, each of its type, as `name`.
        fn attached(&self, name: &str, files: &[(&str, &str)]) -> PathBuf {
            let mut args: Vec<String> = FILMED.iter().map(|a| a.to_string()).collect();
            args.extend(["-c:v", "mpeg4", "-q:v", "2"].map(String::from));
            for (file, _) in files {
                args.extend(["-attach".to_string(), file.to_string()]);
            }
            for (n, (file, kind)) in files.iter().enumerate() {
                args.extend([format!("-metadata:s:t:{n}"), format!("mimetype={kind}"), format!("-metadata:s:t:{n}"), format!("filename={file}")]);
            }
            args.push(name.to_string());
            self.run(&args.iter().map(String::as_str).collect::<Vec<_>>());
            self.path(name)
        }
    }

    impl Drop for Made {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn middle(still: &Still) -> [u8; 3] {
        let at = ((still.height / 2 * still.width + still.width / 2) * 4) as usize;
        [still.rgba[at], still.rgba[at + 1], still.rgba[at + 2]]
    }

    fn near(a: [u8; 3], b: [u8; 3]) -> bool {
        (0..3).all(|c| (a[c] as i32 - b[c] as i32).abs() <= 30)
    }

    /// One stream as Spatiand's FFmpeg gives it: its kind, whether it is an attached picture, and
    /// its `filename`, `mimetype` and `comment` tags.
    #[derive(Debug)]
    struct Seen {
        kind: ffi::AVMediaType,
        attached: bool,
        filename: Option<String>,
        mimetype: Option<String>,
        comment: Option<String>,
    }

    fn streams(path: &Path) -> Vec<Seen> {
        still::opened_with(still::file_name(path).unwrap(), &[], |format| unsafe {
            let streams = std::slice::from_raw_parts((*format).streams, (*format).nb_streams as usize);
            let seen = streams.iter().map(|&s| {
                let m = (*s).metadata;
                Seen {
                    kind: (*(*s).codecpar).codec_type,
                    attached: (*s).disposition & ffi::AV_DISPOSITION_ATTACHED_PIC as c_int != 0,
                    filename: tag(m, c"filename"),
                    mimetype: tag(m, c"mimetype"),
                    comment: tag(m, c"comment"),
                }
            });
            Some(seen.collect())
        })
        .unwrap()
    }

    /// The stream [`chosen`] picks, of the file opened.
    fn chosen_in(path: &Path) -> Option<usize> {
        still::opened_with(still::file_name(path).unwrap(), &[], |format| unsafe { chosen(format) })
    }

    #[test]
    fn which_picture_is_the_cover() {
        // Matroska's four, by name whatever their case: the portrait cover, then the landscape
        // one, then the small ones.
        assert_eq!(rank(Some("cover.jpg"), None), 0);
        assert_eq!(rank(Some("Cover.PNG"), None), 0);
        assert_eq!(rank(Some("cover_land.png"), None), 1);
        assert_eq!(rank(Some("small_cover.jpg"), None), 2);
        assert_eq!(rank(Some("small_cover_land.jpg"), None), 3);
        // Other names a cover goes by, then any other picture.
        assert_eq!(rank(Some("poster.jpg"), None), 4);
        assert_eq!(rank(Some("Film - front.jpg"), None), 4);
        assert_eq!(rank(Some("fanart.jpg"), None), 5);
        // ID3's, FLAC's and Vorbis comments' kinds, and an APE tag's key.
        assert_eq!(rank(None, Some("Cover (front)")), 0);
        assert_eq!(rank(None, Some("Cover Art (Front)")), 0);
        assert_eq!(rank(None, Some("Other")), 4);
        assert_eq!(rank(None, Some("Cover (back)")), 5);
        assert_eq!(rank(None, Some("Lead artist/lead performer/soloist")), 5);
        assert_eq!(rank(None, Some("32x32 pixels 'file icon'")), 6);
        // MP4's covr says nothing of itself: it is the cover.
        assert_eq!(rank(None, None), 0);
        // What a picture's bytes begin with.
        assert_eq!(sniffed(&[0xff, 0xd8, 0xff, 0xe0, 0, 0]), Some(ffi::AV_CODEC_ID_MJPEG));
        assert_eq!(sniffed(b"\x89PNG\r\n\x1a\n...."), Some(ffi::AV_CODEC_ID_PNG));
        assert_eq!(sniffed(b"RIFF\x10\0\0\0WEBPVP8 "), Some(ffi::AV_CODEC_ID_WEBP));
        assert_eq!(sniffed(b"GIF89a.."), Some(ffi::AV_CODEC_ID_GIF));
        assert_eq!(sniffed(b"\0\x01\0\0 a font"), None);
        assert!(pictured_name("cover.JPG") && pictured_name("a.webp") && !pictured_name("DejaVuSans.ttf"));
    }

    /// mkvtoolnix's four covers attached to a film -- the portrait one last, so that its name
    /// and not its place chooses it: that one, no wider than asked and never made larger. A
    /// library's tile of the film is the cover, not a frame of it.
    #[test]
    fn a_matroska_films_portrait_cover_is_chosen_of_its_four() {
        let Some(m) = Made::new("mkv-four") else { return };
        m.picture("cover_land.jpg", GREEN, 450, 300);
        m.picture("small_cover.jpg", BLUE, 80, 120);
        m.picture("small_cover_land.jpg", YELLOW, 120, 80);
        m.picture("cover.jpg", RED, 300, 450);
        let jpeg = "image/jpeg";
        let film = m.attached("film.mkv", &[("cover_land.jpg", jpeg), ("small_cover.jpg", jpeg), ("small_cover_land.jpg", jpeg), ("cover.jpg", jpeg)]);
        // FFmpeg 9.1: each image/jpeg attachment an attached picture -- a video stream -- by name.
        let seen = streams(&film);
        let attached: Vec<&Seen> = seen.iter().filter(|s| s.attached).collect();
        assert_eq!(attached.len(), 4, "{seen:?}");
        assert!(attached.iter().all(|s| s.kind == ffi::AVMEDIA_TYPE_VIDEO && s.mimetype.as_deref() == Some(jpeg)), "{seen:?}");
        assert_eq!(attached[3].filename.as_deref(), Some("cover.jpg"));

        let found = find_file(&film, 320).expect("its cover");
        assert_eq!((found.width, found.height), (300, 450));
        assert!(near(middle(&found), RED), "{:?}", middle(&found));
        let smaller = find_file(&film, 200).unwrap();
        assert_eq!((smaller.width, smaller.height), (200, 300));
        let tile = tile_file(&film, 320).unwrap();
        assert!(near(middle(&tile), RED), "the tile is the cover: {:?}", middle(&tile));
        // Played as a song is, it would be the same picture.
        assert_eq!(chosen_in(&film), seen.iter().position(|s| s.filename.as_deref() == Some("cover.jpg")));
    }

    /// Without the portrait cover, the large landscape one before the small portrait one: a
    /// tile of 120 pixels made three times as large is a blur.
    #[test]
    fn without_the_portrait_cover_the_large_landscape_one_comes_before_the_small() {
        let Some(m) = Made::new("mkv-land") else { return };
        m.picture("small_cover.png", BLUE, 80, 120);
        m.picture("cover_land.png", GREEN, 450, 300);
        let film = m.attached("film.mkv", &[("small_cover.png", "image/png"), ("cover_land.png", "image/png")]);
        let found = find_file(&film, 480).expect("a cover");
        assert_eq!((found.width, found.height), (450, 300));
        assert!(near(middle(&found), GREEN), "{:?}", middle(&found));
    }

    /// A cover attached as a type FFmpeg does not take for a picture -- WebP, a JPEG called
    /// application/octet-stream -- is an attachment stream with its bytes as extradata: read from
    /// those. A font beside it is not mistaken for one.
    #[test]
    fn a_matroska_cover_ffmpeg_keeps_as_an_attachment_is_read_from_its_bytes() {
        let Some(m) = Made::new("mkv-attachment") else { return };
        m.picture("cover.webp", RED, 300, 450);
        std::fs::write(m.path("font.ttf"), b"\0\x01\0\0\0\x0a\0\x80\0\x03\0\x20OS/2 not really a font").unwrap();
        let film = m.attached("webp.mkv", &[("font.ttf", "application/x-truetype-font"), ("cover.webp", "image/webp")]);
        let seen = streams(&film);
        let webp = seen.iter().find(|s| s.filename.as_deref() == Some("cover.webp")).expect("the cover's stream");
        assert_eq!((webp.kind, webp.attached), (ffi::AVMEDIA_TYPE_ATTACHMENT, false), "{seen:?}");
        let found = find_file(&film, 320).expect("the WebP cover");
        assert_eq!((found.width, found.height), (300, 450));
        assert!(near(middle(&found), RED), "{:?}", middle(&found));
        // An attachment is no stream a player can show: played as a song, none is chosen.
        assert_eq!(chosen_in(&film), None);

        // A JPEG whose type says nothing of it, known by its name and its bytes.
        m.picture("cover.jpg", BLUE, 300, 450);
        let film = m.attached("unnamed.mkv", &[("cover.jpg", "application/octet-stream")]);
        assert_eq!(streams(&film).iter().filter(|s| s.kind == ffi::AVMEDIA_TYPE_ATTACHMENT).count(), 1);
        let found = find_file(&film, 320).expect("the JPEG cover");
        assert!(near(middle(&found), BLUE), "{:?}", middle(&found));
    }

    /// A JPEG cover whose EXIF says it is to be turned (6: a quarter turn clockwise), as a phone's
    /// photograph made a cover would: shown turned, as FFmpeg's decoder says by a display matrix.
    #[test]
    fn a_cover_is_put_the_right_way_up_as_its_exif_says() {
        let Some(m) = Made::new("exif") else { return };
        // Red on the left, blue on the right, 300 by 150 as stored.
        m.run(&["-f", "lavfi", "-i", "color=c=0xdc2828:s=150x150", "-f", "lavfi", "-i", "color=c=0x2846dc:s=150x150", "-filter_complex", "hstack", "-frames:v", "1", "-update", "1", "-q:v", "2", "stored.jpg"]);
        let stored = std::fs::read(m.path("stored.jpg")).unwrap();
        // An APP1 "Exif" segment, big-endian TIFF, one entry: Orientation (0x0112), SHORT, 6.
        let mut exif = vec![0xff, 0xe1, 0x00, 0x22];
        exif.extend(b"Exif\0\0MM\0\x2a\0\0\0\x08");
        exif.extend([0x00, 0x01, 0x01, 0x12, 0x00, 0x03, 0x00, 0x00, 0x00, 0x01, 0x00, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
        let turned = [&stored[..2], &exif[..], &stored[2..]].concat();
        std::fs::write(m.path("cover.jpg"), turned).unwrap();
        let film = m.attached("turned.mkv", &[("cover.jpg", "image/jpeg")]);
        let found = find_file(&film, 320).expect("its cover");
        // Turned a quarter clockwise: the left, red, at the top.
        assert_eq!((found.width, found.height), (150, 300));
        let at = |x: u32, y: u32| {
            let i = ((y * found.width + x) * 4) as usize;
            [found.rgba[i], found.rgba[i + 1], found.rgba[i + 2]]
        };
        assert!(near(at(75, 60), RED) && near(at(75, 240), BLUE), "{:?} {:?}", at(75, 60), at(75, 240));
    }

    /// A film with a font attached and no cover: no cover, and its tile is a frame of it.
    #[test]
    fn a_film_with_only_a_font_attached_shows_a_frame_of_itself() {
        let Some(m) = Made::new("mkv-font") else { return };
        std::fs::write(m.path("font.ttf"), b"\0\x01\0\0\0\x0a\0\x80\0\x03\0\x20OS/2 not really a font").unwrap();
        let film = m.attached("fonts.mkv", &[("font.ttf", "application/x-truetype-font")]);
        assert!(find_file(&film, 320).is_none());
        let tile = tile_file(&film, 320).expect("a frame");
        assert_eq!((tile.width, tile.height), (320, 180));
        assert!(near(middle(&tile), FILM), "{:?}", middle(&tile));
    }

    /// An MP4 film's covr, as AtomicParsley, MP4Box and Subler write it, and an M4A song's:
    /// each an attached picture, the cover.
    #[test]
    fn an_mp4_films_covr_and_an_m4a_songs_are_their_covers() {
        let Some(m) = Made::new("mp4") else { return };
        let cover = m.picture("cover.png", RED, 300, 450);
        let mut args: Vec<&str> = FILMED.to_vec();
        args.extend(["-i", &cover, "-map", "0", "-map", "1", "-c:v:0", "mpeg4", "-c:v:1", "copy", "-disposition:v:1", "attached_pic", "film.mp4"]);
        m.run(&args);
        let film = m.path("film.mp4");
        let seen = streams(&film);
        assert_eq!(seen.iter().filter(|s| s.attached).count(), 1, "{seen:?}");
        let found = find_file(&film, 320).expect("the covr");
        assert_eq!((found.width, found.height), (300, 450));
        assert!(near(middle(&found), RED), "{:?}", middle(&found));
        assert!(near(middle(&tile_file(&film, 320).unwrap()), RED));

        let jpeg = m.picture("cover.jpg", GREEN, 400, 400);
        let mut args: Vec<&str> = SOUNDED.to_vec();
        args.extend(["-i", &jpeg, "-map", "0", "-map", "1", "-c:a", "aac", "-c:v", "copy", "-disposition:v", "attached_pic", "song.m4a"]);
        m.run(&args);
        let song = m.path("song.m4a");
        assert!(streams(&song).iter().any(|s| s.attached));
        let found = tile_file(&song, 320).expect("the song's covr");
        assert_eq!((found.width, found.height), (320, 320));
        assert!(near(middle(&found), GREEN), "{:?}", middle(&found));
    }

    /// An MP3's APIC frames: of a back cover and a front cover, the front, wherever it is; and
    /// with only a picture of ID3's kind "Other", that one.
    #[test]
    fn an_mp3s_front_cover_is_chosen_over_its_back_cover() {
        let Some(m) = Made::new("mp3") else { return };
        let back = m.picture("back.jpg", BLUE, 300, 300);
        let front = m.picture("front.jpg", RED, 300, 300);
        let mut args: Vec<&str> = SOUNDED.to_vec();
        args.extend(["-i", &back, "-i", &front, "-map", "0", "-map", "1", "-map", "2", "-c:a", "libmp3lame", "-c:v", "copy", "-id3v2_version", "3"]);
        args.extend(["-metadata:s:v:0", "comment=Cover (back)", "-metadata:s:v:1", "comment=Cover (front)", "song.mp3"]);
        m.run(&args);
        let song = m.path("song.mp3");
        let seen = streams(&song);
        let kinds: Vec<&str> = seen.iter().filter(|s| s.attached).filter_map(|s| s.comment.as_deref()).collect();
        assert_eq!(kinds, ["Cover (back)", "Cover (front)"], "{seen:?}");
        let found = tile_file(&song, 320).expect("the front cover");
        assert!(near(middle(&found), RED), "{:?}", middle(&found));
        // A song plays with the same picture: the front cover's stream -- where FFmpeg, asked for
        // the best picture stream as the player asks, takes the back one, the first of two alike.
        assert_eq!(chosen_in(&song), seen.iter().position(|s| s.comment.as_deref() == Some("Cover (front)")));
        let ffmpegs = still::opened_with(still::file_name(&song).unwrap(), &[], |format| unsafe {
            ffi::avformat_find_stream_info(format, ptr::null_mut());
            let best = ffi::av_find_best_stream(format, ffi::AVMEDIA_TYPE_VIDEO, -1, -1, ptr::null_mut(), 0);
            (best >= 0).then_some(best as usize)
        });
        assert_eq!(ffmpegs, seen.iter().position(|s| s.comment.as_deref() == Some("Cover (back)")));

        let mut args: Vec<&str> = SOUNDED.to_vec();
        args.extend(["-i", &back, "-map", "0", "-map", "1", "-c:a", "libmp3lame", "-c:v", "copy", "other.mp3"]);
        m.run(&args);
        let other = m.path("other.mp3");
        assert_eq!(streams(&other).iter().find(|s| s.attached).and_then(|s| s.comment.clone()).as_deref(), Some("Other"));
        assert!(near(middle(&tile_file(&other, 320).expect("its only picture")), BLUE));
    }

    /// FLAC's picture block as Vorbis comments carry it, in base64: METADATA_BLOCK_PICTURE.
    fn picture_block(kind: u32, mime: &str, picture: &[u8], w: u32, h: u32) -> String {
        let mut block = Vec::new();
        block.extend(kind.to_be_bytes());
        block.extend((mime.len() as u32).to_be_bytes());
        block.extend(mime.as_bytes());
        // No description; then its size, its depth, no palette, and the picture.
        for number in [0, w, h, 24, 0, picture.len() as u32] {
            block.extend(number.to_be_bytes());
        }
        block.extend(picture);
        const LETTERS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut text = String::new();
        for three in block.chunks(3) {
            let n = (three[0] as u32) << 16 | (*three.get(1).unwrap_or(&0) as u32) << 8 | *three.get(2).unwrap_or(&0) as u32;
            for i in 0..4 {
                text.push(if i <= three.len() { LETTERS[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' });
            }
        }
        text
    }

    /// A FLAC's PICTURE block, and an Ogg Opus's and Vorbis's METADATA_BLOCK_PICTURE comment.
    #[test]
    fn a_flacs_picture_and_an_oggs_picture_comment_are_their_covers() {
        let Some(m) = Made::new("flac-ogg") else { return };
        let cover = m.picture("cover.png", RED, 300, 300);
        let mut args: Vec<&str> = SOUNDED.to_vec();
        args.extend(["-i", &cover, "-map", "0", "-map", "1", "-c:a", "flac", "-c:v", "copy", "-disposition:v", "attached_pic", "-metadata:s:v", "comment=Cover (front)", "song.flac"]);
        m.run(&args);
        let song = m.path("song.flac");
        assert_eq!(streams(&song).iter().find(|s| s.attached).and_then(|s| s.comment.clone()).as_deref(), Some("Cover (front)"));
        let found = tile_file(&song, 320).expect("the FLAC's picture");
        assert!(near(middle(&found), RED), "{:?}", middle(&found));

        let png = std::fs::read(m.path("cover.png")).unwrap();
        let comment = format!("METADATA_BLOCK_PICTURE={}", picture_block(3, "image/png", &png, 300, 300));
        for (codec, name) in [("libopus", "song.opus"), ("libvorbis", "song.ogg")] {
            let mut args: Vec<&str> = SOUNDED.to_vec();
            args.extend(["-c:a", codec, "-metadata:s:a:0", &comment, name]);
            m.run(&args);
            let song = m.path(name);
            let seen = streams(&song);
            assert!(seen.iter().any(|s| s.attached && s.comment.as_deref() == Some("Cover (front)")), "{name}: {seen:?}");
            let found = tile_file(&song, 320).unwrap_or_else(|| panic!("{name}: its picture"));
            assert!(near(middle(&found), RED), "{name}: {:?}", middle(&found));
        }
    }

    /// A film without a cover: none found, and its tile is a frame of it. A song without one:
    /// no tile at all -- its kind's mark is shown instead.
    #[test]
    fn a_film_without_a_cover_shows_a_frame_and_a_song_without_one_nothing() {
        let Some(m) = Made::new("none") else { return };
        let mut args: Vec<&str> = FILMED.to_vec();
        args.extend(["-c:v", "mpeg4", "-q:v", "2", "plain.mp4"]);
        m.run(&args);
        let film = m.path("plain.mp4");
        assert!(find_file(&film, 320).is_none());
        let frame = tile_file(&film, 320).expect("a frame");
        assert_eq!((frame.width, frame.height), (320, 180));
        assert!(near(middle(&frame), FILM), "{:?}", middle(&frame));
        let mut args: Vec<&str> = SOUNDED.to_vec();
        args.extend(["-c:a", "libmp3lame", "plain.mp3"]);
        m.run(&args);
        assert!(tile_file(&m.path("plain.mp3"), 320).is_none());
        assert!(tile(m.path("plain.mp3").to_str().unwrap(), 320).is_none());
    }

    /// A file served: its address, the ranges asked for, and the bytes sent.
    struct Served {
        address: String,
        asked: Arc<Mutex<Vec<String>>>,
        sent: Arc<Mutex<u64>>,
    }

    /// A server of one file on 127.0.0.1 that answers a `Range` with its part, as the storage
    /// service does, and counts what it sends: what the socket took, its own buffer kept small so
    /// that what was never read is hardly counted.
    fn server(file: PathBuf) -> Served {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let name = file.file_name().unwrap().to_string_lossy().into_owned();
        let address = format!("http://{}/dav/scratch/{name}", listener.local_addr().unwrap());
        let (asked, sent) = (Arc::new(Mutex::new(Vec::new())), Arc::new(Mutex::new(0u64)));
        let (asked_, sent_) = (asked.clone(), sent.clone());
        std::thread::spawn(move || {
            let whole = Arc::new(std::fs::read(&file).unwrap());
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                // Each connection on its own, as a real server's: FFmpeg asks for the place it
                // seeks to before it lets go of the connection it was reading.
                let (whole, asked, sent) = (whole.clone(), asked_.clone(), sent_.clone());
                std::thread::spawn(move || answer(stream, &whole, &asked, &sent));
            }
        });
        Served { address, asked, sent }
    }

    fn answer(mut stream: std::net::TcpStream, whole: &[u8], asked: &Mutex<Vec<String>>, sent: &Mutex<u64>) {
        let small: libc::c_int = 16 * 1024;
        unsafe {
            use std::os::fd::AsRawFd;
            let size = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
            libc::setsockopt(stream.as_raw_fd(), libc::SOL_SOCKET, libc::SO_SNDBUF, &small as *const libc::c_int as *const libc::c_void, size);
        }
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            if stream.read(&mut byte).unwrap_or(0) == 0 {
                return;
            }
            head.push(byte[0]);
        }
        let head = String::from_utf8_lossy(&head).into_owned();
        let range = head.lines().find_map(|line| line.to_ascii_lowercase().strip_prefix("range: bytes=").map(str::to_string));
        asked.lock().unwrap().push(range.clone().unwrap_or_default());
        let last = whole.len() as u64 - 1;
        let (from, to) = match range.as_deref().and_then(|r| r.split_once('-')) {
            Some((from, to)) => (from.parse::<u64>().unwrap_or(0), to.parse::<u64>().map_or(last, |to| to.min(last))),
            None => (0, last),
        };
        let reply = format!(
            "HTTP/1.1 206 Partial Content\r\nContent-Type: application/octet-stream\r\nAccept-Ranges: bytes\r\nContent-Range: bytes {from}-{to}/{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            whole.len(),
            to + 1 - from
        );
        if stream.write_all(reply.as_bytes()).is_err() {
            return;
        }
        for piece in whole[from as usize..=to as usize].chunks(16 * 1024) {
            if stream.write_all(piece).is_err() {
                return;
            }
            *sent.lock().unwrap() += piece.len() as u64;
        }
    }

    /// A cover on a server -- in a Matroska film's header, and in an MP4's index at its end --
    /// and a frame of a film with none, each read from a file of more than 40 MB without the file
    /// being fetched: under a fortieth of it sent, asked for a megabyte at a time while the
    /// header is read.
    #[test]
    fn a_cover_on_a_server_is_read_without_the_whole_file_being_fetched() {
        let Some(m) = Made::new("server") else { return };
        let cover = m.picture("cover.jpg", RED, 300, 450);
        // Two minutes of loud noise at 192 kHz, sound no codec makes smaller: 46 MB as PCM in a
        // Matroska film, 69 MB as FLAC in an MP4 one (whose index ffmpeg puts at the end).
        let film = ["-f", "lavfi", "-i", "color=c=0x9664e6:s=320x180:r=5:d=120", "-f", "lavfi", "-i", "anoisesrc=d=120:r=192000:a=1"];
        let mut args: Vec<&str> = film.to_vec();
        args.extend(["-c:v", "mpeg4", "-c:a", "pcm_s16le", "-attach", &cover, "-metadata:s:t:0", "mimetype=image/jpeg", "big.mkv"]);
        m.run(&args);
        let mut args: Vec<&str> = film.to_vec();
        args.extend(["-i", &cover, "-map", "0", "-map", "1", "-map", "2", "-c:v:0", "mpeg4", "-c:a", "flac", "-compression_level", "0", "-c:v:1", "copy", "-disposition:v:1", "attached_pic", "big.mp4"]);
        m.run(&args);
        let mut args: Vec<&str> = film.to_vec();
        args.extend(["-c:v", "mpeg4", "-q:v", "2", "-c:a", "pcm_s16le", "plain.mkv"]);
        m.run(&args);
        for (name, colour, cover) in [("big.mkv", RED, true), ("big.mp4", RED, true), ("plain.mkv", FILM, false)] {
            let path = m.path(name);
            let size = path.metadata().unwrap().len();
            assert!(size > 40_000_000, "{name}: {size}");
            let served = server(path);
            let still = tile(&served.address, 320).unwrap_or_else(|| panic!("{name}: a picture"));
            // The connections are closed: what the server goes on trying to send is refused.
            std::thread::sleep(std::time::Duration::from_millis(200));
            let (asked, sent) = (served.asked.lock().unwrap().clone(), *served.sent.lock().unwrap());
            eprintln!("{name}: {size} bytes; {sent} sent; asked for {asked:?}");
            assert!(near(middle(&still), colour), "{name}: {:?}", middle(&still));
            assert_eq!(still.height > still.width, cover, "{name}: {}x{}", still.width, still.height);
            // Not the file: under a fortieth of it (a megabyte or so of 46).
            assert!(sent < size / 40, "{name}: {sent} of {size} bytes sent");
            // Asked for a megabyte at a time while the header was read, never for all the rest.
            assert!(asked.iter().all(|range| range.split_once('-').is_some_and(|(from, to)| to.parse::<u64>().is_ok_and(|to| to < from.parse::<u64>().unwrap_or(0) + (1 << 20)))), "{name}: {asked:?}");
        }
    }
}
