# Third-party software in Spatiand

Spatiand's own code is MIT. It ships, as separate libraries and programs beside its own, the
software below, each under its own licence. This file goes inside every build (on the Mac,
`Spatiand.app/Contents/Resources/licences/`), with the licence texts beside it.

| What | Version | Licence | Where its source is |
|---|---|---|---|
| **FFmpeg** (libavcodec, libavformat, libavutil, libavfilter, libavdevice, libswscale, libswresample) | 9.1 | **LGPL 3.0 or later** (`COPYING.LGPLv3`) | https://ffmpeg.org/releases/ffmpeg-9.1.tar.xz, built by `third_party/ffmpeg/build.sh` (its configure line is there) with the patch in `third_party/ffmpeg/patches/ffmpeg/` (its TLS code on BoringSSL) |
| **BoringSSL** (libssl, libcrypto), inside FFmpeg's libraries, for its https | the revision SpatiMesh's Chromium pins; each build names the commit it was built from (`third_party/ffmpeg/out/<platform>/.version`) | Apache 2.0 (`BoringSSL-LICENSE.txt`) | https://boringssl.googlesource.com/boringssl at the `boringssl_revision` of that Chromium's `DEPS`, with SpatiMesh's own patch to it (`third_party/chromium/patches/sub/boringssl/`): the checkout in SpatiMesh's Chromium tree, built by `third_party/ffmpeg/build.sh` |
| rsmpeg (Rust bindings to FFmpeg, compiled into the programs) | 0.18.0, patched | MIT | `third_party/rsmpeg/` in Spatiand's repository, with `PATCHES.md` |
| PDFium | chromium/8152 | BSD 3-clause and others | https://github.com/bblanchon/pdfium-binaries |
| ANGLE (libEGL, libGLESv2) | see `mac/fetch-angle.sh` | BSD 3-clause | https://chromium.googlesource.com/angle/angle |
| libmysofa | 1.3.5 | BSD 3-clause (`mysofa-NOTICE.txt`) | https://github.com/hoene/libmysofa/releases/tag/v1.3.5 |
| rclone (in the storage service, linked in as a Go module, unchanged: no `replace` in `storage/go.mod`) | v1.76.0 | MIT | https://github.com/rclone/rclone/releases/tag/v1.76.0 (the module and its dependencies: `storage/go.mod`, `storage/go.sum`) |

**FFmpeg is LGPL, and linked dynamically**: the FFmpeg libraries in the app are ordinary shared
libraries that can be replaced with a build of one's own of the same version. It is configured
without `--enable-gpl` and without `--enable-nonfree`, and with `--enable-version3` because
BoringSSL, built into its libraries, is under the Apache licence, which is compatible with LGPL 3,
not 2.1. The build refuses to finish if FFmpeg ever comes out GPL or non-free.

## SpatiMesh (third_party/chromium)

SpatiMesh is built from Chromium's source (fetched at build time, not kept here) with the patches
in `third_party/chromium`:

| What | Where | Licence | From |
|---|---|---|---|
| **Chromium** | fetched by `third_party/chromium/build.sh` at the pinned version | BSD 3-Clause and the licences of its parts (`chrome://credits` in the built browser) | https://chromium.googlesource.com/chromium/src |
| **Cromite's patches**, taken over at Chromium 153 and carried by this project since | `third_party/chromium/patches/base`, `patches/sub` | **GPL-3.0** (`patches/base/COPYING`) | https://github.com/uazo/cromite, commit 3724efc; it gathers Bromite's and ungoogled-chromium's among its own |
| **eyeo's Chromium SDK** (the ad blocker), inside those patches | same | **GPL-3.0** | https://gitlab.com/eyeo/adblockplus/chromium-sdk |
| **termux-packages' Chromium patches** | `third_party/chromium/android-libc/termux` | Chromium's licence (a package's patches carry the package's licence) | https://github.com/termux/termux-packages, commit 47d0a63 |
| **Spatiand's own patches** | `third_party/chromium/patches/0*.patch`, `android-libc/0*.patch`, `mac/` | MIT, as the rest of Spatiand; where one changes a GPL file of the above, that file stays GPL-3.0 | this repository |

**SpatiMesh as built is therefore under the GPL-3.0.** Its complete corresponding source is the
pinned Chromium plus these patches, applied by `third_party/chromium/build.sh`. The ad blocker's
filter lists are fetched by the running browser from their maintainers and are not part of it.
