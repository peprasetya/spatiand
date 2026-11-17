//! Android's own decoders (MediaCodec) under the player, as VideoToolbox is under it on a Mac and
//! VA-API on the Deck, and the way their pictures stay on the GPU.
//!
//! FFmpeg is asked for MediaCodec by name (`h264_mediacodec` and the rest: they are decoders of
//! their own, not a hardware mode of FFmpeg's own decoders), on a device whose window is an
//! `AImageReader`'s ([`Reader`]): the codec draws each picture into one of the reader's Android
//! hardware buffers (`AIMAGE_FORMAT_PRIVATE`, YUV in the GPU's own layout), and the screen samples
//! that buffer as an external texture, the GPU's own conversion making it colour
//! (`spatiand_app::gl::HardwareBufferTexture`). Nothing is copied, as nothing is on the Deck.
//!
//! **A picture is drawn into the reader only once the one before has been taken from it.** A
//! codec's window does not wait for the reader: a picture drawn while the one before is still in
//! the reader's queue replaces it (SpatiMesh found that on the Beam Pro, 2026-11-17, 60-frame
//! H.264 reaching the page at 42 a second). The picture thread draws one, waits for it to arrive,
//! and goes on; the codec meanwhile decodes the next.
//!
//! What needs no phone -- which decoder, whether a codec is the hardware's, whether a picture in
//! the reader is the one drawn, where the picture is in its buffer -- is here for every platform,
//! and tested on any.

/// FFmpeg's MediaCodec decoder for a codec (by FFmpeg's own name for the codec), and the MIME
/// type Android names it by.
pub fn decoder_for(codec: &str) -> Option<(&'static str, &'static str)> {
    Some(match codec {
        "h264" => ("h264_mediacodec", "video/avc"),
        "hevc" => ("hevc_mediacodec", "video/hevc"),
        "vp9" => ("vp9_mediacodec", "video/x-vnd.on2.vp9"),
        "vp8" => ("vp8_mediacodec", "video/x-vnd.on2.vp8"),
        "av1" => ("av1_mediacodec", "video/av01"),
        "mpeg2video" => ("mpeg2_mediacodec", "video/mpeg2"),
        "mpeg4" => ("mpeg4_mediacodec", "video/mp4v-es"),
        _ => return None,
    })
}

/// Whether a MediaCodec decoder of this name is one of Android's own in software (as SpatiMesh's
/// Chromium judges them, android-libc/0015): those are left to FFmpeg's own, which is as good.
/// No name is software too: nothing is known of it.
pub fn is_software(name: &str) -> bool {
    name.is_empty() || name.starts_with("c2.android.") || name.starts_with("OMX.google.") || name.contains(".sw.")
}

/// What a picture taken from the reader is to the one the codec was told to draw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Taken {
    /// It: shown.
    Drawn,
    /// One from before (drawn before a seek, or that came after its wait was given up): let go,
    /// and the next one looked at.
    Earlier,
}

/// Whether the reader's picture, stamped `nanoseconds` (MediaCodec stamps what it draws with the
/// picture's presentation time), is the one drawn for `expected` (microseconds, as FFmpeg gave
/// the codec the packet's time; none where the film has no times). FFmpeg's times go to the codec
/// and back through two roundings, so a microsecond or two either way is the same picture. One
/// later than expected is taken too: the one expected never arrived, and this is the next.
pub fn judge(nanoseconds: i64, expected: Option<i64>) -> Taken {
    match expected {
        Some(expected) if nanoseconds.div_euclid(1000) < expected - 2 => Taken::Earlier,
        _ => Taken::Drawn,
    }
}

/// The part of a buffer `size` big that is picture, as the reader's crop rectangle gives it
/// (left, top, right, bottom in pixels), in the texture's coordinates: left, top, right, bottom,
/// each 0 to 1. A rectangle that is empty or outside the buffer is the whole buffer.
pub fn crop_in(rect: [i32; 4], size: (u32, u32)) -> [f32; 4] {
    let (w, h) = (size.0 as i32, size.1 as i32);
    let [left, top, right, bottom] = rect;
    if w <= 0 || h <= 0 || left < 0 || top < 0 || right <= left || bottom <= top || right > w || bottom > h {
        return [0.0, 0.0, 1.0, 1.0];
    }
    [left as f32 / w as f32, top as f32 / h as f32, right as f32 / w as f32, bottom as f32 / h as f32]
}

#[cfg(all(target_os = "android", feature = "engine"))]
pub use phone::*;

#[cfg(all(target_os = "android", feature = "engine"))]
mod phone {
    use std::collections::HashMap;
    use std::ffi::{c_void, CStr, CString};
    use std::ptr;
    use std::sync::{Arc, Mutex, OnceLock};
    use std::time::{Duration, Instant};

    use ndk_sys as ndk;
    use rsmpeg::ffi;

    use super::{crop_in, judge, Taken};

    /// Pictures out of the codec at once: those waiting to be shown (`PICTURES_AHEAD`), the one
    /// on show and the one before it while the screen lets go of it, the one being taken, and two
    /// to spare. The codec draws into buffers of its own besides these.
    const READER_IMAGES: i32 = 10;

    /// Whether this phone decodes `mime` with a hardware decoder: the decoder Android gives for
    /// it, by its name. Asked once a type.
    pub fn hardware_decodes(mime: &str) -> bool {
        static ANSWERS: OnceLock<Mutex<HashMap<String, bool>>> = OnceLock::new();
        let answers = ANSWERS.get_or_init(Default::default);
        if let Some(answer) = answers.lock().unwrap().get(mime) {
            return *answer;
        }
        let answer = (|| {
            let named = CString::new(mime).ok()?;
            let codec = unsafe { ndk::AMediaCodec_createDecoderByType(named.as_ptr()) };
            if codec.is_null() {
                return None;
            }
            let mut name: *mut std::ffi::c_char = ptr::null_mut();
            let found = unsafe { ndk::AMediaCodec_getName(codec, &mut name) } == ndk::media_status_t::AMEDIA_OK && !name.is_null();
            let text = if found { unsafe { CStr::from_ptr(name) }.to_string_lossy().into_owned() } else { String::new() };
            if found {
                unsafe { ndk::AMediaCodec_releaseName(codec, name) };
            }
            unsafe { ndk::AMediaCodec_delete(codec) };
            log::info!("{mime}: Android's decoder is {}", if text.is_empty() { "unnamed" } else { &text });
            Some(!super::is_software(&text))
        })()
        .unwrap_or(false);
        answers.lock().unwrap().insert(mime.to_string(), answer);
        answer
    }

    /// `libavutil/hwcontext_mediacodec.h`, which the bindings do not carry.
    #[repr(C)]
    struct MediaCodecDevice {
        surface: *mut c_void,
        native_window: *mut c_void,
        create_window: std::ffi::c_int,
    }

    /// An FFmpeg MediaCodec device whose pictures go to `reader`.
    pub unsafe fn device(reader: &Reader) -> Result<*mut ffi::AVBufferRef, String> {
        let mut device = ffi::av_hwdevice_ctx_alloc(ffi::AV_HWDEVICE_TYPE_MEDIACODEC);
        if device.is_null() {
            return Err("FFmpeg has no MediaCodec device".into());
        }
        let context = (*device).data as *mut ffi::AVHWDeviceContext;
        let wanted = (*context).hwctx as *mut MediaCodecDevice;
        (*wanted).native_window = reader.window as *mut c_void;
        let made = ffi::av_hwdevice_ctx_init(device);
        if made < 0 {
            ffi::av_buffer_unref(&mut device);
            return Err(format!("FFmpeg's MediaCodec device would not start ({made})"));
        }
        Ok(device)
    }

    /// How often the reader is looked at while a picture is on its way: it comes within a
    /// millisecond or two of being drawn.
    const LOOK_EVERY: Duration = Duration::from_micros(500);

    /// The `AImageReader` a film's decoder draws into, and its pictures as they arrive.
    ///
    /// **Looked at, not listened to.** A listener's call comes on the reader's own thread, and
    /// can still be under way while the reader is deleted: closing a film then ended the program
    /// ("pthread_mutex_lock called on a destroyed mutex", the listener's context freed under it).
    pub struct Reader {
        reader: *mut ndk::AImageReader,
        window: *mut ndk::ANativeWindow,
    }

    // The reader is the NDK's, which takes calls from any thread; a picture is taken on the
    // picture thread alone.
    unsafe impl Send for Reader {}
    unsafe impl Sync for Reader {}

    impl Reader {
        /// A reader for pictures `size` big: only the size of buffers nobody sized, as the
        /// codec sizes its own.
        pub fn new(size: (u32, u32)) -> Result<Arc<Reader>, String> {
            let mut reader = ptr::null_mut();
            let status = unsafe {
                ndk::AImageReader_newWithUsage(
                    size.0.max(16) as i32,
                    size.1.max(16) as i32,
                    ndk::AIMAGE_FORMATS::AIMAGE_FORMAT_PRIVATE.0 as i32,
                    ndk::AHardwareBuffer_UsageFlags::AHARDWAREBUFFER_USAGE_GPU_SAMPLED_IMAGE.0,
                    READER_IMAGES,
                    &mut reader,
                )
            };
            if status != ndk::media_status_t::AMEDIA_OK || reader.is_null() {
                return Err(format!("no image reader for the film: {status:?}"));
            }
            let mut window = ptr::null_mut();
            if unsafe { ndk::AImageReader_getWindow(reader, &mut window) } != ndk::media_status_t::AMEDIA_OK || window.is_null() {
                unsafe { ndk::AImageReader_delete(reader) };
                return Err("the film's image reader has no window".into());
            }
            Ok(Arc::new(Reader { reader, window }))
        }

        /// The picture the codec was just told to draw, stamped `expected` (microseconds): the
        /// reader's pictures from before it are let go. None if it has not come within
        /// `patience`.
        pub fn take(self: &Arc<Self>, expected: Option<i64>, patience: Duration) -> Option<Image> {
            let until = Instant::now() + patience;
            loop {
                let mut image = ptr::null_mut();
                let status = unsafe { ndk::AImageReader_acquireNextImage(self.reader, &mut image) };
                if status == ndk::media_status_t::AMEDIA_OK && !image.is_null() {
                    let mut nanoseconds = 0i64;
                    unsafe { ndk::AImage_getTimestamp(image, &mut nanoseconds) };
                    if judge(nanoseconds, expected) == Taken::Earlier {
                        unsafe { ndk::AImage_delete(image) };
                        continue;
                    }
                    return Image::new(image, self.clone());
                }
                if status == ndk::media_status_t::AMEDIA_IMGREADER_MAX_IMAGES_ACQUIRED {
                    log::warn!("every picture of the film's reader is held; one is skipped");
                    return None;
                }
                // Nothing yet.
                if Instant::now() >= until {
                    return None;
                }
                std::thread::sleep(LOOK_EVERY);
            }
        }
    }

    impl Drop for Reader {
        fn drop(&mut self) {
            unsafe { ndk::AImageReader_delete(self.reader) };
        }
    }

    /// One picture in the reader, the codec's again when dropped.
    pub struct Image {
        image: *mut ndk::AImage,
        /// The buffer the picture is in.
        pub buffer: *mut ndk::AHardwareBuffer,
        /// The part of the buffer that is picture, in the texture's coordinates: left, top,
        /// right, bottom.
        pub crop: [f32; 4],
        /// Keeps the reader for as long as its picture is held.
        _reader: Arc<Reader>,
    }

    // An AImage is let go from whichever thread drops the picture; the NDK allows it.
    unsafe impl Send for Image {}
    unsafe impl Sync for Image {}

    impl Image {
        fn new(image: *mut ndk::AImage, reader: Arc<Reader>) -> Option<Image> {
            let mut buffer = ptr::null_mut();
            if unsafe { ndk::AImage_getHardwareBuffer(image, &mut buffer) } != ndk::media_status_t::AMEDIA_OK || buffer.is_null() {
                unsafe { ndk::AImage_delete(image) };
                return None;
            }
            let mut described = ndk::AHardwareBuffer_Desc { width: 0, height: 0, layers: 0, format: 0, usage: 0, stride: 0, rfu0: 0, rfu1: 0 };
            unsafe { ndk::AHardwareBuffer_describe(buffer, &mut described) };
            let mut rect = ndk::AImageCropRect { left: 0, top: 0, right: 0, bottom: 0 };
            let cropped = unsafe { ndk::AImage_getCropRect(image, &mut rect) } == ndk::media_status_t::AMEDIA_OK;
            let crop = if cropped { crop_in([rect.left, rect.top, rect.right, rect.bottom], (described.width, described.height)) } else { [0.0, 0.0, 1.0, 1.0] };
            Some(Image { image, buffer, crop, _reader: reader })
        }
    }

    impl Drop for Image {
        fn drop(&mut self) {
            unsafe { ndk::AImage_delete(self.image) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_codecs_android_decodes_have_ffmpegs_decoders() {
        assert_eq!(decoder_for("h264"), Some(("h264_mediacodec", "video/avc")));
        assert_eq!(decoder_for("hevc"), Some(("hevc_mediacodec", "video/hevc")));
        assert_eq!(decoder_for("vp9").map(|d| d.1), Some("video/x-vnd.on2.vp9"));
        assert_eq!(decoder_for("av1").map(|d| d.0), Some("av1_mediacodec"));
        // Nothing Android decodes for FFmpeg's own: those stay FFmpeg's.
        assert_eq!(decoder_for("prores"), None);
        assert_eq!(decoder_for("mjpeg"), None);
    }

    #[test]
    fn androids_own_software_decoders_are_known_by_name() {
        assert!(!is_software("c2.qti.avc.decoder"));
        assert!(!is_software("c2.qti.hevc.decoder"));
        assert!(!is_software("OMX.qcom.video.decoder.avc"));
        assert!(is_software("c2.android.av1.decoder"));
        assert!(is_software("OMX.google.h264.decoder"));
        assert!(is_software("c2.vendor.sw.hevc.decoder"));
        assert!(is_software(""));
    }

    #[test]
    fn the_picture_drawn_is_the_one_stamped_with_its_time() {
        // 1/90000 to microseconds and back, as FFmpeg sends a packet's time and returns it.
        let pts = 3003i64;
        let micros = (pts * 1_000_000 + 45_000) / 90_000; // 33367
        assert_eq!(judge(micros * 1000, Some(micros)), Taken::Drawn);
        // A microsecond either way is the same picture.
        assert_eq!(judge((micros - 1) * 1000, Some(micros)), Taken::Drawn);
        assert_eq!(judge((micros + 1) * 1000 + 999, Some(micros)), Taken::Drawn);
        // A frame before it, still in the reader from before a seek, is let go.
        assert_eq!(judge((micros - 16_683) * 1000, Some(micros)), Taken::Earlier);
        assert_eq!(judge(0, Some(micros)), Taken::Earlier);
        // A later one is taken: the one expected never came.
        assert_eq!(judge((micros + 16_683) * 1000, Some(micros)), Taken::Drawn);
        // A film without times takes whatever comes.
        assert_eq!(judge(0, None), Taken::Drawn);
    }

    #[test]
    fn the_picture_is_found_inside_its_buffer() {
        // 1080 rows of picture in a buffer of 1088, as decoders align them.
        let crop = crop_in([0, 0, 1920, 1080], (1920, 1088));
        assert_eq!(crop[0..3], [0.0, 0.0, 1.0]);
        assert!((crop[3] - 1080.0 / 1088.0).abs() < 1e-6);
        // Nothing said, or nonsense: the whole buffer.
        assert_eq!(crop_in([0, 0, 0, 0], (1920, 1088)), [0.0, 0.0, 1.0, 1.0]);
        assert_eq!(crop_in([0, 0, 4000, 1080], (1920, 1088)), [0.0, 0.0, 1.0, 1.0]);
        assert_eq!(crop_in([0, 0, 1920, 1080], (0, 0)), [0.0, 0.0, 1.0, 1.0]);
    }
}
