# The Android NDK every native piece of the Beam Pro is built with, named once, here: the room and
# its apps (android/build.sh, cargo-ndk), eglshim, xkbcommon, mysofa, FFmpeg for Android
# (third_party/ffmpeg), SpatiMesh's libraries for Android's libc (third_party/chromium/android-libc/
# deps.sh) and the pieces of it SpatiMesh's own build takes (third_party/chromium/build.sh: the
# unwinder, the atomics, llvm-strip), and what CI installs (tools/ci-deps.sh).
#
# Sourced, not run:   . android/ndk.sh   -> SPATIAND_NDK (the NDK's folder), SPATIAND_NDK_REVISION
#
# A new NDK: change SPATIAND_NDK_VERSION below (its Pkg.Revision, as the SDK's ndk/ folder names it)
# and put that NDK at $ANDROID_HOME/ndk/<version> (sdkmanager "ndk;<version>", or a link to an
# unpacked release). Or set SPATIAND_NDK to an NDK's folder to build with that one instead. Programs
# built with another NDK are rebuilt by the scripts above when SPATIAND_NDK_REVISION changes.
SPATIAND_NDK_VERSION=30.0.16248370   # r30, 2026-09-08
spatiand_sdk=${ANDROID_HOME:-$(case "$(uname -s)" in (Darwin) echo "$HOME/Library/Android/sdk" ;; (*) echo "$HOME/Android/Sdk" ;; esac)}
SPATIAND_NDK=${SPATIAND_NDK:-$spatiand_sdk/ndk/$SPATIAND_NDK_VERSION}
unset spatiand_sdk
if [ ! -f "$SPATIAND_NDK/source.properties" ]; then
    echo "no Android NDK at $SPATIAND_NDK (android/ndk.sh: SPATIAND_NDK, or NDK $SPATIAND_NDK_VERSION in the SDK)" >&2
    exit 1
fi
SPATIAND_NDK_REVISION=$(sed -n 's/^Pkg\.Revision *= *//p' "$SPATIAND_NDK/source.properties")
