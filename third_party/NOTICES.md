# Third-party software in Spatiand

Spatiand's own code is MIT. It ships, as separate libraries and programs beside its own, the
software below, each under its own licence. This file goes inside every build (on the Mac,
`Spatiand.app/Contents/Resources/licences/`), with the licence texts beside it.

| What | Version | Licence | Where its source is |
|---|---|---|---|
| **FFmpeg** (libavcodec, libavformat, libavutil, libavfilter, libavdevice, libswscale, libswresample) | 9.0.2 | **LGPL 3.0 or later** (`COPYING.LGPLv3`) | https://ffmpeg.org/releases/ffmpeg-9.0.2.tar.xz, built unchanged by `third_party/ffmpeg/build.sh` (its configure line is there; patches, if any, in `third_party/ffmpeg/patches/`) |
| **OpenSSL** (libssl, libcrypto), used by FFmpeg for https | 3.5.9 | Apache 2.0 (`OpenSSL-LICENSE.txt`) | https://github.com/openssl/openssl/releases/tag/openssl-3.5.9 |
| rsmpeg (Rust bindings to FFmpeg, compiled into the programs) | 0.18.0, patched | MIT | `third_party/rsmpeg/` in Spatiand's repository, with `PATCHES.md` |
| PDFium | chromium/8100 | BSD 3-clause and others | https://github.com/bblanchon/pdfium-binaries |
| ANGLE (libEGL, libGLESv2) | see `mac/fetch-angle.sh` | BSD 3-clause | https://chromium.googlesource.com/angle/angle |
| libmysofa | commit 648eed0 | BSD 3-clause (`mysofa-NOTICE.txt`) | https://github.com/hoene/libmysofa |
| rclone (in the storage service) | Spatiand's fork | MIT | `storage/` in Spatiand's repository |

**FFmpeg is LGPL, and linked dynamically**: the FFmpeg libraries in the app are ordinary shared
libraries that can be replaced with a build of one's own of the same version. It is configured
without `--enable-gpl` and without `--enable-nonfree`, and with `--enable-version3` because
OpenSSL's Apache licence is compatible with LGPL 3, not 2.1. The build refuses to finish if
FFmpeg ever comes out GPL or non-free.
