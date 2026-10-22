#!/bin/sh
# FFmpeg as Spatiand ships it: one version on every platform (versions.sh), built here from
# FFmpeg's own release, with each platform's hardware decoder. **No software AV1** (dav1d): AV1
# is played where the hardware decodes it -- FFmpeg's own AV1 decoder hands it to the hardware --
# and not on the processor where it does not (the owner's choice, 2026-10-22).
#
#   third_party/ffmpeg/build.sh mac|android|linux      (linux: the Deck, and CI)
#
# **LGPL only** (version 3, for OpenSSL): no --enable-gpl, no --enable-nonfree, none of the
# libraries that would make it GPL (x264, x265). It is linked dynamically, so Spatiand's own code stays MIT; the libraries go
# beside the programs, with the notice (third_party/NOTICES.md says where the source is).
#
# Lands in third_party/ffmpeg/out/<platform>: include/ and lib/pkgconfig/ for the bindings,
# lib/ for the app. Sources are kept in src/, so a rebuild does not fetch them again; our own
# changes, if ever there are any, go in patches/ffmpeg and patches/openssl as numbered files and
# are applied in order -- a new version is a changed number in versions.sh and those re-applied.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
. "$here/versions.sh"
platform=${1:?mac, android or linux}
out=$here/out/$platform
src=$here/src
stamp="ffmpeg $FFMPEG_VERSION openssl $OPENSSL_VERSION"
if [ -f "$out/.version" ] && [ "$(cat "$out/.version")" = "$stamp" ]; then
    exit 0
fi
mkdir -p "$src"
jobs=$( (sysctl -n hw.ncpu || nproc) 2>/dev/null )

fetch() { # name url
    [ -d "$src/$1" ] && return
    case $2 in *.gz) unpack=-xz ;; *) unpack=-xJ ;; esac
    curl -fsSL "$2" | tar $unpack -C "$src"
    for p in "$here/patches/${1%%-*}"/*.patch; do
        [ -f "$p" ] && (cd "$src/$1" && patch -p1 < "$p")
    done
    return 0
}
fetch "ffmpeg-$FFMPEG_VERSION" "https://ffmpeg.org/releases/ffmpeg-$FFMPEG_VERSION.tar.xz"
fetch "openssl-$OPENSSL_VERSION" "https://github.com/openssl/openssl/releases/download/openssl-$OPENSSL_VERSION/openssl-$OPENSSL_VERSION.tar.gz"

rm -rf "$out"
mkdir -p "$out"
build=$here/build/$platform
rm -rf "$build"
mkdir -p "$build"

# What every platform's FFmpeg has: decoding and demuxing for the apps, https by
# OpenSSL, shared. **Nothing found by chance**: a library that happened to be installed where it
# was built (Homebrew's liblzma, X11) was linked, and would be missing on any other machine. So
# nothing is autodetected, and each platform names what it uses.
common="--prefix=$out --enable-shared --disable-static --disable-programs --disable-doc
    --disable-debug --disable-indevs --disable-outdevs --enable-pic --disable-autodetect
    --enable-version3 --enable-openssl --enable-zlib"

# OpenSSL, built shared into the same place (Linux: the system's own).
openssl() { # Configure-target [args]
    (cd "$src/openssl-$OPENSSL_VERSION" && make distclean >/dev/null 2>&1 || true)
    mkdir -p "$build/openssl"
    (cd "$build/openssl" && "$src/openssl-$OPENSSL_VERSION/Configure" "$@" shared no-tests no-docs no-apps \
        --prefix="$out" --libdir=lib && make -j"$jobs" && make install_sw)
}

case $platform in
mac)
    min=14.0
    flags="-mmacosx-version-min=$min"
    case "$(uname -m)" in arm64) target=darwin64-arm64-cc ;; *) target=darwin64-x86_64-cc ;; esac
    openssl "$target" "$flags"
    install_name_tool -id @rpath/libcrypto.3.dylib "$out/lib/libcrypto.3.dylib"
    install_name_tool -id @rpath/libssl.3.dylib -change "$out/lib/libcrypto.3.dylib" @rpath/libcrypto.3.dylib "$out/lib/libssl.3.dylib"
    (cd "$src/ffmpeg-$FFMPEG_VERSION" && make distclean >/dev/null 2>&1 || true)
    (cd "$build" && PKG_CONFIG_PATH="$out/lib/pkgconfig" "$src/ffmpeg-$FFMPEG_VERSION/configure" $common \
        --enable-videotoolbox --enable-audiotoolbox --enable-bzlib --enable-iconv \
        --install-name-dir=@rpath \
        --extra-cflags="$flags" --extra-ldflags="$flags -Wl,-rpath,@loader_path" --extra-libs=-liconv)
    ;;
android)
    SDK=${ANDROID_HOME:-$HOME/Library/Android/sdk}
    NDK=${ANDROID_NDK_HOME:-$SDK/ndk/28.2.13676358}
    HOST_TAG=$(case "$(uname -s)" in (Darwin) echo darwin-x86_64 ;; (*) echo linux-x86_64 ;; esac)
    TOOLS=$NDK/toolchains/llvm/prebuilt/$HOST_TAG/bin
    API=34
    # Inside FFmpeg's own libraries, not beside them: an APK carries only files named lib*.so,
    # and OpenSSL's are named with their version (libssl.so.3).
    (cd "$src/openssl-$OPENSSL_VERSION" && make distclean >/dev/null 2>&1 || true)
    mkdir -p "$build/openssl"
    (cd "$build/openssl" && PATH="$TOOLS:$PATH" ANDROID_NDK_ROOT="$NDK" "$src/openssl-$OPENSSL_VERSION/Configure" \
        android-arm64 -D__ANDROID_API__=$API no-shared no-tests no-docs no-apps -fPIC \
        --prefix="$out" --libdir=lib && PATH="$TOOLS:$PATH" make -j"$jobs" && make install_sw)
    (cd "$build" && PKG_CONFIG_PATH="$out/lib/pkgconfig" PKG_CONFIG_LIBDIR="$out/lib/pkgconfig" "$src/ffmpeg-$FFMPEG_VERSION/configure" $common \
        --target-os=android --arch=aarch64 --cpu=armv8-a --enable-cross-compile \
        --cc="$TOOLS/aarch64-linux-android$API-clang" --cxx="$TOOLS/aarch64-linux-android$API-clang++" \
        --ar="$TOOLS/llvm-ar" --nm="$TOOLS/llvm-nm" --ranlib="$TOOLS/llvm-ranlib" --strip="$TOOLS/llvm-strip" \
        --sysroot="$NDK/toolchains/llvm/prebuilt/$HOST_TAG/sysroot" --pkg-config=pkg-config \
        --enable-jni --enable-mediacodec \
        --extra-ldflags="-Wl,-z,max-page-size=16384")
    ;;
linux)
    # The system's OpenSSL, libva and Opus (the voice sent to a host is Opus, and FFmpeg's own
    # encoder is experimental): each a library every Linux the Deck or CI is has already.
    (cd "$build" && "$src/ffmpeg-$FFMPEG_VERSION/configure" $common \
        --enable-vaapi --enable-libdrm --enable-bzlib --enable-libopus \
        --extra-ldflags="-Wl,-rpath,\$\$ORIGIN")
    ;;
*) echo "no such platform: $platform" >&2; exit 1 ;;
esac
# Said loudly if it ever comes out otherwise: Spatiand does not ship a GPL FFmpeg.
grep -q '^#define CONFIG_GPL 0' "$build/config.h" && grep -q '^#define CONFIG_NONFREE 0' "$build/config.h" \
    || { echo "FFmpeg was configured GPL or nonfree; refusing" >&2; exit 1; }
make -C "$build" -j"$jobs"
make -C "$build" install
echo "$stamp" > "$out/.version"
echo "$stamp for $platform in $out"
