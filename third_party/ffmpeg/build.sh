#!/bin/sh
# FFmpeg as Spatiand ships it: one version on every platform (versions.sh), built here from
# FFmpeg's own release, with each platform's hardware decoder. **No software AV1** (dav1d): AV1
# is played where the hardware decodes it -- FFmpeg's own AV1 decoder hands it to the hardware --
# and not on the processor where it does not (the owner's choice, 2026-10-22).
#
#   third_party/ffmpeg/build.sh mac|android|linux      (linux: the Deck, and CI)
#
# **LGPL only** (version 3, for BoringSSL's Apache licence): no --enable-gpl, no --enable-nonfree,
# none of the libraries that would make it GPL (x264, x265). It is linked dynamically, so Spatiand's own code stays MIT; the libraries go
# beside the programs, with the notice (third_party/NOTICES.md says where the source is).
#
# **https by BoringSSL** (the owner, 2026-11-26: "what if ffmpeg use boringssl?"), SpatiMesh's
# own, read from the source of its Chromium (versions.sh) and built here, inside FFmpeg's own
# libraries with its names hidden there: nothing is needed beside FFmpeg for it, and it meets no
# other TLS library in a process -- the BoringSSL inside SpatiMesh, the Deck's OpenSSL.
#
# Lands in third_party/ffmpeg/out/<platform>: include/ and lib/pkgconfig/ for the bindings,
# lib/ for the app, share/boringssl/LICENSE for the licences. FFmpeg's source is kept in src/, so a
# rebuild does not fetch it again; our own changes go in patches/ffmpeg as numbered files and are
# applied in order -- a new version is a changed number in versions.sh and those re-applied.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
. "$here/versions.sh"
src=$here/src
fetch() { # name url
    [ -d "$src/$1" ] && return
    # Unpacked and patched aside, and moved into place whole: a download cut short (ffmpeg.org
    # stalls at times; a stall of a minute ends it) leaves nothing a later run takes for the source.
    part=$(mktemp -d "$src/.fetch.XXXXXX")
    curl -fsSL --speed-limit 1024 --speed-time 60 "$2" | tar -xJ -C "$part"
    for p in "$here/patches/${1%%-*}"/*.patch; do
        [ -f "$p" ] && (cd "$part/$1" && patch -p1 < "$p")
    done
    [ -d "$src/$1" ] || mv "$part/$1" "$src/$1"
    rm -rf "$part"
}
# `build.sh fetch`: the source alone, for a build that cannot reach the network itself --
# third_party/chromium/build.sh builds this inside a root with no name lookup, so it fetches first,
# on the machine.
if [ "${1:-}" = fetch ]; then
    mkdir -p "$src"
    fetch "ffmpeg-$FFMPEG_VERSION" "https://ffmpeg.org/releases/ffmpeg-$FFMPEG_VERSION.tar.xz"
    exit 0
fi
platform=${1:?mac, android, linux or fetch}
out=$here/out/$platform
# Which BoringSSL: its checkout's commit, and a checksum of whatever is changed in it and not
# committed, so that the stamp names what was built and another one is a rebuild.
top=$(git -C "$BORINGSSL_SRC" rev-parse --show-toplevel 2>/dev/null || true)
if [ -z "$top" ] || [ "$top" != "$(cd "$BORINGSSL_SRC" && pwd -P)" ]; then
    echo "no git checkout of BoringSSL at $BORINGSSL_SRC: SpatiMesh's Chromium tree has one, or SPATIAND_BORINGSSL names another (versions.sh)" >&2
    exit 1
fi
boringssl=$(git -C "$BORINGSSL_SRC" rev-parse HEAD)
[ -z "$(git -C "$BORINGSSL_SRC" status --porcelain --untracked-files=no)" ] ||
    boringssl="$boringssl+$(git -C "$BORINGSSL_SRC" diff HEAD | cksum | cut -d' ' -f1)"
stamp="ffmpeg $FFMPEG_VERSION boringssl $boringssl"
# For Android, also the NDK it is built with (android/ndk.sh): another one is a rebuild.
if [ "$platform" = android ]; then
    . "$here/../../android/ndk.sh"
    stamp="$stamp ndk $SPATIAND_NDK_REVISION"
fi
if [ -f "$out/.version" ] && [ "$(cat "$out/.version")" = "$stamp" ]; then
    exit 0
fi
command -v cmake >/dev/null || { echo "BoringSSL is built with CMake (3.22 or later): none on the PATH" >&2; exit 1; }
mkdir -p "$src"
jobs=$( (sysctl -n hw.ncpu || nproc) 2>/dev/null )

fetch "ffmpeg-$FFMPEG_VERSION" "https://ffmpeg.org/releases/ffmpeg-$FFMPEG_VERSION.tar.xz"

rm -rf "$out"
mkdir -p "$out"
build=$here/build/$platform
rm -rf "$build"
mkdir -p "$build"

# What every platform's FFmpeg has: decoding and demuxing for the apps, https by
# BoringSSL (FFmpeg's OpenSSL code: BoringSSL keeps OpenSSL's API, and patches/ffmpeg/0001 the
# rest), shared. **Nothing found by chance**: a library that happened to be installed where it
# was built (Homebrew's liblzma, X11) was linked, and would be missing on any other machine. So
# nothing is autodetected, and each platform names what it uses.
common="--prefix=$out --enable-shared --disable-static --disable-programs --disable-doc
    --disable-debug --disable-indevs --disable-outdevs --enable-pic --disable-autodetect
    --enable-version3 --enable-openssl --enable-zlib"

# BoringSSL as SpatiMesh's Chromium builds it, without C++ exceptions or RTTI; static, its code
# position-independent, to go inside FFmpeg's shared libraries; its names hidden (BoringSSL's own
# default). Its C++ then asks the C++ runtime only for operator delete and a throw of
# out_of_range, which each platform below has its own C++ library give. Built in the build
# folder; the tree it is read from is not written to.
build_boringssl() { # [CMake arguments for the platform]
    CXXFLAGS="-fno-exceptions -fno-rtti" cmake -S "$BORINGSSL_SRC" -B "$build/boringssl" \
        $(command -v ninja >/dev/null && echo -GNinja) -DCMAKE_BUILD_TYPE=Release -DBUILD_TESTING=OFF \
        -DCMAKE_POSITION_INDEPENDENT_CODE=ON "$@"
    cmake --build "$build/boringssl" --target ssl crypto -j "$jobs"
}
# What FFmpeg's configure asks pkg-config for as openssl: BoringSSL's folder first, where there
# are its archives and nothing else, then the C++ runtime. (By their paths the archives come
# before what needs them in configure's test, which then finds nothing in them.) Its version is
# that of the OpenSSL whose API BoringSSL keeps (its OPENSSL_VERSION_NUMBER).
openssl_pc() { # C++ runtime
    mkdir -p "$build/pkgconfig"
    cat > "$build/pkgconfig/openssl.pc" <<EOF
Name: openssl
Description: BoringSSL $boringssl, SpatiMesh's
Version: 1.1.1
Cflags: -I$BORINGSSL_SRC/include
Libs: -L$build/boringssl -lssl -lcrypto $*
EOF
}

case $platform in
mac)
    min=14.0
    flags="-mmacosx-version-min=$min"
    # BoringSSL for this Mac's processor and macOS 14 on; for its C++, the system's libc++.
    build_boringssl -DCMAKE_OSX_DEPLOYMENT_TARGET=$min -DCMAKE_OSX_ARCHITECTURES="$(uname -m)"
    openssl_pc -lc++
    (cd "$src/ffmpeg-$FFMPEG_VERSION" && make distclean >/dev/null 2>&1 || true)
    (cd "$build" && PKG_CONFIG_PATH="$build/pkgconfig" "$src/ffmpeg-$FFMPEG_VERSION/configure" $common \
        --enable-videotoolbox --enable-audiotoolbox --enable-bzlib --enable-iconv \
        --install-name-dir=@rpath \
        --extra-cflags="$flags" --extra-ldflags="$flags -Wl,-rpath,@loader_path" --extra-libs=-liconv)
    ;;
android)
    NDK=$SPATIAND_NDK
    HOST_TAG=$(case "$(uname -s)" in (Darwin) echo darwin-x86_64 ;; (*) echo linux-x86_64 ;; esac)
    TOOLS=$NDK/toolchains/llvm/prebuilt/$HOST_TAG/bin
    API=34
    # BoringSSL by the NDK's own CMake toolchain, inside FFmpeg's own libraries as everything here
    # is: an APK carries only files named lib*.so. For its C++, the NDK's static libc++, inside
    # them too and hidden as BoringSSL is: Android has no libc++ for an app to share.
    build_boringssl -DCMAKE_TOOLCHAIN_FILE="$NDK/build/cmake/android.toolchain.cmake" \
        -DANDROID_ABI=arm64-v8a -DANDROID_PLATFORM=android-$API
    openssl_pc -lc++_static -lc++abi
    (cd "$build" && PKG_CONFIG_PATH="$build/pkgconfig" PKG_CONFIG_LIBDIR="$build/pkgconfig" "$src/ffmpeg-$FFMPEG_VERSION/configure" $common \
        --target-os=android --arch=aarch64 --cpu=armv8-a --enable-cross-compile \
        --cc="$TOOLS/aarch64-linux-android$API-clang" --cxx="$TOOLS/aarch64-linux-android$API-clang++" \
        --ar="$TOOLS/llvm-ar" --nm="$TOOLS/llvm-nm" --ranlib="$TOOLS/llvm-ranlib" --strip="$TOOLS/llvm-strip" \
        --sysroot="$NDK/toolchains/llvm/prebuilt/$HOST_TAG/sysroot" --pkg-config=pkg-config \
        --enable-jni --enable-mediacodec \
        --extra-ldflags="-Wl,-z,max-page-size=16384 -Wl,--exclude-libs,ALL")
    ;;
linux)
    # The system's libva and Opus (the voice sent to a host is Opus, and FFmpeg's own encoder is
    # experimental), and its C++ runtime (libstdc++, libgcc_s) for BoringSSL's: each a library
    # every Linux the Deck or CI is has already. On deepmagpie, for SpatiMesh, built inside the
    # Ubuntu 22.04 root its Chromium is built in (third_party/chromium/build.sh), whose programs
    # load it while they build: one built on deepmagpie itself asks for its glibc, 2.43. On the
    # Deck, inside its build container, which has no libva or Opus headers and no nasm: SteamOS's
    # headers and nasm are in ~/spatiand-build, so
    #   distrobox enter holo -- env PKG_CONFIG_PATH=$HOME/spatiand-build/pkgconfig \
    #       PATH=$HOME/spatiand-build/nasm/bin:/usr/bin:/bin nice -n 19 sh third_party/ffmpeg/build.sh linux
    # (BoringSSL: its copy there, versions.sh).
    build_boringssl
    openssl_pc -lstdc++
    (cd "$build" && PKG_CONFIG_PATH="$build/pkgconfig${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}" "$src/ffmpeg-$FFMPEG_VERSION/configure" $common \
        --enable-vaapi --enable-libdrm --enable-bzlib --enable-libopus \
        --extra-ldflags=-Wl,--exclude-libs,ALL)
    # No search path in the libraries themselves ($ORIGIN does not survive configure and make
    # alike on every shell): a program finds them by its own, set as DT_RPATH, which -- unlike
    # RUNPATH -- also serves the libraries it loads (the Deck: its .cargo/config.toml; CI:
    # LD_LIBRARY_PATH).
    ;;
*) echo "no such platform: $platform" >&2; exit 1 ;;
esac
# Said loudly if it ever comes out otherwise: Spatiand does not ship a GPL FFmpeg; and its https
# is the BoringSSL built here, where configure would take an OpenSSL of the system's, unasked,
# if it did not find that one (it did so on deepmagpie, 2026-11-27, and said nothing).
grep -q '^#define CONFIG_GPL 0' "$build/config.h" && grep -q '^#define CONFIG_NONFREE 0' "$build/config.h" \
    || { echo "FFmpeg was configured GPL or nonfree; refusing" >&2; exit 1; }
grep -qF -- "-L$build/boringssl " "$build/ffbuild/config.mak" \
    || { echo "FFmpeg was configured with an OpenSSL of the system's, not the BoringSSL built here; refusing" >&2; exit 1; }
make -C "$build" -j"$jobs"
make -C "$build" install
# And if a name of BoringSSL's is ever exported from FFmpeg's libraries, where it could meet
# another TLS library's in a process.
case $platform in
mac) exported() { nm -gU "$out"/lib/lib*.dylib; } ;;
android) exported() { "$TOOLS/llvm-nm" -D --defined-only "$out"/lib/lib*.so; } ;;
*) exported() { nm -D --defined-only "$out"/lib/lib*.so; } ;;
esac
if exported | grep -E ' _?(SSL|BIO|EVP|X509|PEM|ASN1|RAND|ERR|BN|EC|RSA|CRYPTO|OPENSSL|CBS|CBB)_|bssl'; then
    echo "BoringSSL's names are exported from FFmpeg's libraries; refusing" >&2
    exit 1
fi
mkdir -p "$out/share/boringssl"
cp "$BORINGSSL_SRC/LICENSE" "$out/share/boringssl/LICENSE"
echo "$stamp" > "$out/.version"
echo "$stamp for $platform in $out"
