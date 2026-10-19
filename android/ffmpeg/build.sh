#!/bin/sh
# FFmpeg for the Beam Pro: the engine SpatiReel, SpatiLoupe and SpatiShelf decode with
# (crates/spatiand-media), built once with the NDK, shared, with Android's own hardware decoder
# (MediaCodec) in it. The same version the Deck's headers are, 7.1. Lands in ffmpeg/out:
# include/ for the bindings, lib/ for the APK.
set -eu
cd "$(dirname "$0")"
[ -f out/lib/libavcodec.so ] && exit 0
VERSION=7.1.1
SDK=${ANDROID_HOME:-$HOME/Library/Android/sdk}
NDK=${ANDROID_NDK_HOME:-$SDK/ndk/28.2.13676358}
# The NDK's tools are in a folder named for the machine building: a Mac's or Linux's.
HOST_TAG=$(case "$(uname -s)" in (Darwin) echo darwin-x86_64 ;; (*) echo linux-x86_64 ;; esac)
TOOLS=$NDK/toolchains/llvm/prebuilt/$HOST_TAG/bin
API=34
[ -d "ffmpeg-$VERSION" ] || curl -fsSL "https://ffmpeg.org/releases/ffmpeg-$VERSION.tar.xz" | tar -xJ
cd "ffmpeg-$VERSION"
./configure \
    --prefix="$(cd .. && pwd)/out" \
    --target-os=android --arch=aarch64 --cpu=armv8-a --enable-cross-compile \
    --cc="$TOOLS/aarch64-linux-android$API-clang" --cxx="$TOOLS/aarch64-linux-android$API-clang++" \
    --ar="$TOOLS/llvm-ar" --nm="$TOOLS/llvm-nm" --ranlib="$TOOLS/llvm-ranlib" --strip="$TOOLS/llvm-strip" \
    --sysroot="$NDK/toolchains/llvm/prebuilt/$HOST_TAG/sysroot" \
    --enable-shared --disable-static --disable-programs --disable-doc --disable-debug \
    --disable-postproc --disable-indevs --disable-outdevs \
    --enable-jni --enable-mediacodec --enable-pic \
    --extra-ldflags="-Wl,-z,max-page-size=16384"
make -j8
make install
echo "FFmpeg $VERSION for Android in $(cd .. && pwd)/out"
