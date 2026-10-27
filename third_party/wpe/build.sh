#!/bin/sh
# WPE WebKit for SpatiMesh, built from its release with Spatiand's own patches (patches/<name>/,
# numbered, applied in order) -- on the Mac, where WebKit upstream builds only as Apple's own
# WebKit, WPE is ported here.
#
#   third_party/wpe/build.sh mac
#   third_party/wpe/build.sh linux              # a bundle that runs on SteamOS and other distros
#   SYSTEM_DEPS=1 third_party/wpe/build.sh linux  # on the distro's own libraries, for developing
#   third_party/wpe/build.sh android            # ANDROID_NDK_HOME set: arm64 for the Beam Pro
#
# On Linux the bundle is built on Ubuntu 22.04 (as Spatiand's SteamOS builds are: its glibc and
# libstdc++ are the oldest it is to run on) and carries every library WPE needs but those every
# desktop Linux has: glibc and libstdc++, Mesa's EGL and GLES, OpenSSL 3, zlib, bzip2, expat,
# libffi, libva and PulseAudio's client. Its libraries find each other by $ORIGIN. On 22.04:
#
#   apt install build-essential gcc-12 g++-12 cmake ninja-build python3-venv perl ruby gperf \
#       unifdef bison flex nasm patchelf pkg-config curl xz-utils git libssl-dev libva-dev \
#       libpulse-dev zlib1g-dev libbz2-dev libexpat1-dev libffi-dev libegl-dev libgles-dev
#
# **Built outside the repository**, in $WPE_WORK (~/Developer/spatiand-wpe by default): a WebKit
# build is tens of gigabytes, and the repository is in ~/Documents, which iCloud copies. The
# result is $WPE_WORK/out/<platform>; the app is put together from there.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
. "$here/versions.sh"
platform=${1:?mac}
work=${WPE_WORK:-$HOME/Developer/spatiand-wpe}
src=$work/src
out=$work/out/$platform
build=$work/build/$platform
mkdir -p "$src" "$out" "$build"
jobs=${JOBS:-$( (sysctl -n hw.ncpu || nproc) 2>/dev/null )}
# meson and ninja for the GNOME libraries, from a folder of this script's own.
[ -x "$work/.venv/bin/meson" ] || { python3 -m venv "$work/.venv" && "$work/.venv/bin/pip" install -q meson ninja; }
PATH=$work/.venv/bin:$PATH
export PKG_CONFIG_PATH="$out/lib/pkgconfig${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"

fetch() { # name url
    [ -d "$src/$1" ] && return
    case $2 in *.gz) unpack=-xz ;; *.bz2) unpack=-xj ;; *) unpack=-xJ ;; esac
    curl -fsSL "$2" | tar $unpack -C "$src"
    for p in "$here/patches/${1%-*}"/*.patch; do
        [ -f "$p" ] && (cd "$src/$1" && patch -p1 < "$p")
    done
    return 0
}
cmake_lib() { # name [cmake options]
    name=$1; shift
    [ -f "$build/$name/.installed" ] && return
    cmake -S "$src/$name" -B "$build/$name" -G Ninja -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX="$out" \
        -DCMAKE_INSTALL_LIBDIR=lib -DCMAKE_INSTALL_NAME_DIR="$out/lib" -DCMAKE_PREFIX_PATH="$out" \
        -DCMAKE_IGNORE_PREFIX_PATH="/usr/local;/opt/homebrew" -DBUILD_SHARED_LIBS=ON "$@"
    ninja -C "$build/$name" install
    touch "$build/$name/.installed"
}
auto_lib() { # name [configure options]
    name=$1; shift
    [ -f "$build/$name/.installed" ] && return
    mkdir -p "$build/$name"
    (cd "$build/$name" && "$src/$name/configure" --prefix="$out" --disable-static --enable-shared "$@" && make -j"$jobs" && make install)
    touch "$build/$name/.installed"
}
meson_lib() { # name [meson options]
    name=$1; shift
    [ -f "$build/$name/.installed" ] && return
    meson setup --wipe "$build/$name" "$src/$name" --prefix="$out" --libdir=lib --buildtype=release "$@" \
        || meson setup "$build/$name" "$src/$name" --prefix="$out" --libdir=lib --buildtype=release "$@"
    ninja -C "$build/$name" install
    touch "$build/$name/.installed"
}

# WebKit itself: the whole of its Source at the WPE release's tag, not the WPE release tarball,
# which leaves out what only Apple's ports use -- and on a Mac WPE uses some of it (Mach, the
# Darwin pieces of WTF). A shallow, sparse clone; Spatiand's changes are patches/webkit/.
webkit=$work/webkit-$WPE_VERSION
if [ ! -d "$webkit/Source" ]; then
    git clone -q --depth 1 --branch "wpewebkit-$WPE_VERSION" --filter=blob:none --sparse https://github.com/WebKit/WebKit.git "$webkit"
    git -C "$webkit" sparse-checkout set --no-cone Source/ Tools/Scripts/ Tools/glib/ Tools/wpe/ /CMakeLists.txt /Tools/CMakeLists.txt /Tools/PlatformWPE.cmake
fi
# Each patch not yet in the checkout, in order, and written down in it (.spatiand-patches) once
# it is: a checkout made before a patch was added takes it here. A checkout from before that
# list has the patches up to the last that comes off it in reverse (those before it are under it).
applied=$webkit/.spatiand-patches
if [ ! -f "$applied" ]; then
    : > "$applied.new"
    last=
    for p in "$here"/patches/webkit/*.patch; do
        [ -f "$p" ] && git -C "$webkit" apply --reverse --check "$p" 2>/dev/null && last=$(basename "$p")
    done
    if [ -n "$last" ]; then
        for p in "$here"/patches/webkit/*.patch; do
            basename "$p" >> "$applied.new"
            [ "$(basename "$p")" = "$last" ] && break
        done
    fi
    mv "$applied.new" "$applied"
fi
for p in "$here"/patches/webkit/*.patch; do
    [ -f "$p" ] || continue
    grep -qx "$(basename "$p")" "$applied" && continue
    echo "applying $(basename "$p")"
    git -C "$webkit" apply "$p"
    basename "$p" >> "$applied"
done

case $platform in
mac)
    # The macOS Spatiand runs on, for every library built here (meson takes it from this).
    export MACOSX_DEPLOYMENT_TARGET=14.0
    # **Nothing of Homebrew's**: pkg-config sees only what is built here (and the system's own
    # libraries are found by the compilers without it). A library found by chance is one the
    # app needs and does not carry.
    export PKG_CONFIG_LIBDIR="$out/lib/pkgconfig"
    # Our prefix ahead of clang's built-in /usr/local, where Homebrew's libintl and libpng sit.
    export CPPFLAGS="-I$out/include" LDFLAGS="-L$out/lib"
    export CFLAGS="$CPPFLAGS" CXXFLAGS="$CPPFLAGS"
    unset PKG_CONFIG_PATH
    # Python's own downloads (meson's) trust the system's roots.
    export SSL_CERT_FILE=/etc/ssl/cert.pem

    # What macOS has not got. Its own SQLite, libxml2, zlib, bzip2, expat, libffi and ICU are used.
    gnu=https://ftp.gnu.org/gnu
    fetch "zstd-$ZSTD_VERSION" "https://github.com/facebook/zstd/releases/download/v$ZSTD_VERSION/zstd-$ZSTD_VERSION.tar.gz"
    [ -f "$build/zstd-$ZSTD_VERSION/.installed" ] || { cmake -S "$src/zstd-$ZSTD_VERSION/build/cmake" -B "$build/zstd-$ZSTD_VERSION" -G Ninja \
        -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX="$out" -DCMAKE_INSTALL_LIBDIR=lib -DCMAKE_INSTALL_NAME_DIR="$out/lib" \
        -DZSTD_BUILD_PROGRAMS=OFF -DZSTD_BUILD_STATIC=OFF -DZSTD_BUILD_TESTS=OFF && ninja -C "$build/zstd-$ZSTD_VERSION" install && touch "$build/zstd-$ZSTD_VERSION/.installed"; }
    fetch "brotli-$BROTLI_VERSION" "https://github.com/google/brotli/archive/refs/tags/v$BROTLI_VERSION.tar.gz"
    cmake_lib "brotli-$BROTLI_VERSION" -DBROTLI_DISABLE_TESTS=ON -DBROTLI_BUILD_TOOLS=OFF
    fetch "woff2-$WOFF2_VERSION" "https://github.com/google/woff2/archive/refs/tags/v$WOFF2_VERSION.tar.gz"
    cmake_lib "woff2-$WOFF2_VERSION" -DCMAKE_POLICY_VERSION_MINIMUM=3.5
    fetch "pcre2-$PCRE2_VERSION" "https://github.com/PCRE2Project/pcre2/releases/download/pcre2-$PCRE2_VERSION/pcre2-$PCRE2_VERSION.tar.bz2"
    cmake_lib "pcre2-$PCRE2_VERSION" -DPCRE2_BUILD_PCRE2GREP=OFF -DPCRE2_BUILD_TESTS=OFF -DPCRE2_SUPPORT_JIT=ON
    # gettext's libintl, which WebKit calls; GLib itself is built without it.
    fetch "gettext-$GETTEXT_VERSION" "$gnu/gettext/gettext-$GETTEXT_VERSION.tar.xz"
    [ -f "$build/gettext-$GETTEXT_VERSION/.installed" ] || { mkdir -p "$build/gettext-$GETTEXT_VERSION" && (cd "$build/gettext-$GETTEXT_VERSION" \
        && "$src/gettext-$GETTEXT_VERSION/gettext-runtime/configure" --prefix="$out" --disable-static --enable-shared --disable-java \
            --disable-csharp --disable-libasprintf --without-emacs --with-included-gettext \
        && make -j"$jobs" -C intl && make -C intl install) && touch "$build/gettext-$GETTEXT_VERSION/.installed"; }
    # macOS's own libffi, said to pkg-config (the system has no .pc of it, and meson, not finding
    # it, went to download one).
    sdk=$(xcrun --show-sdk-path)
    mkdir -p "$out/lib/pkgconfig"
    cat > "$out/lib/pkgconfig/libffi.pc" <<PC
Name: libffi
Description: macOS's own
Version: 3.4.0
Cflags: -I$sdk/usr/include/ffi
Libs: -lffi
PC
    # The SDK's zlib, which libpng's .pc requires and macOS ships no .pc of.
    cat > "$out/lib/pkgconfig/zlib.pc" <<PC
Name: zlib
Description: macOS's own
Version: 1.2.12
Libs: -lz
PC
    fetch "glib-$GLIB_VERSION" "https://download.gnome.org/sources/glib/${GLIB_VERSION%.*}/glib-$GLIB_VERSION.tar.xz"
    meson_lib "glib-$GLIB_VERSION" -Dnls=disabled -Dtests=false -Dintrospection=disabled -Ddocumentation=false -Dman-pages=disabled \
        -Dsysprof=disabled -Dlibelf=disabled -Dglib_debug=disabled
    fetch "libpng-$LIBPNG_VERSION" "https://download.sourceforge.net/libpng/libpng-$LIBPNG_VERSION.tar.xz"
    cmake_lib "libpng-$LIBPNG_VERSION" -DPNG_TESTS=OFF -DPNG_TOOLS=OFF -DPNG_STATIC=OFF -DPNG_FRAMEWORK=OFF
    fetch "freetype-$FREETYPE_VERSION" "https://download.savannah.gnu.org/releases/freetype/freetype-$FREETYPE_VERSION.tar.xz"
    meson_lib "freetype-$FREETYPE_VERSION" -Dharfbuzz=disabled -Dbrotli=disabled -Dpng=enabled -Dzlib=system -Dbzip2=enabled -Dtests=disabled
    fetch "fontconfig-$FONTCONFIG_VERSION" "https://gitlab.freedesktop.org/api/v4/projects/890/packages/generic/fontconfig/$FONTCONFIG_VERSION/fontconfig-$FONTCONFIG_VERSION.tar.xz"
    meson_lib "fontconfig-$FONTCONFIG_VERSION" -Ddoc=disabled -Dtests=disabled -Dtools=disabled -Dcache-build=disabled -Dnls=disabled
    fetch "libjpeg-turbo-$LIBJPEG_TURBO_VERSION" "https://github.com/libjpeg-turbo/libjpeg-turbo/releases/download/$LIBJPEG_TURBO_VERSION/libjpeg-turbo-$LIBJPEG_TURBO_VERSION.tar.gz"
    cmake_lib "libjpeg-turbo-$LIBJPEG_TURBO_VERSION" -DENABLE_STATIC=OFF -DWITH_TURBOJPEG=OFF
    fetch "libwebp-$LIBWEBP_VERSION" "https://storage.googleapis.com/downloads.webmproject.org/releases/webp/libwebp-$LIBWEBP_VERSION.tar.gz"
    cmake_lib "libwebp-$LIBWEBP_VERSION" -DWEBP_BUILD_ANIM_UTILS=OFF -DWEBP_BUILD_CWEBP=OFF -DWEBP_BUILD_DWEBP=OFF -DWEBP_BUILD_GIF2WEBP=OFF \
        -DWEBP_BUILD_IMG2WEBP=OFF -DWEBP_BUILD_VWEBP=OFF -DWEBP_BUILD_WEBPINFO=OFF -DWEBP_BUILD_WEBPMUX=OFF -DWEBP_BUILD_EXTRAS=OFF
    fetch "nghttp2-$NGHTTP2_VERSION" "https://github.com/nghttp2/nghttp2/releases/download/v$NGHTTP2_VERSION/nghttp2-$NGHTTP2_VERSION.tar.xz"
    cmake_lib "nghttp2-$NGHTTP2_VERSION" -DENABLE_LIB_ONLY=ON -DBUILD_STATIC_LIBS=OFF -DENABLE_DOC=OFF -DBUILD_TESTING=OFF
    fetch "libpsl-$LIBPSL_VERSION" "https://github.com/rockdaboot/libpsl/releases/download/$LIBPSL_VERSION/libpsl-$LIBPSL_VERSION.tar.gz"
    meson_lib "libpsl-$LIBPSL_VERSION" -Druntime=no -Dtests=false -Ddocs=false
    fetch "libtasn1-$LIBTASN1_VERSION" "$gnu/libtasn1/libtasn1-$LIBTASN1_VERSION.tar.gz"
    auto_lib "libtasn1-$LIBTASN1_VERSION" --disable-doc
    fetch "libgpg-error-$LIBGPG_ERROR_VERSION" "https://gnupg.org/ftp/gcrypt/libgpg-error/libgpg-error-$LIBGPG_ERROR_VERSION.tar.bz2"
    auto_lib "libgpg-error-$LIBGPG_ERROR_VERSION" --disable-doc --disable-tests --disable-nls
    fetch "libgcrypt-$LIBGCRYPT_VERSION" "https://gnupg.org/ftp/gcrypt/libgcrypt/libgcrypt-$LIBGCRYPT_VERSION.tar.bz2"
    auto_lib "libgcrypt-$LIBGCRYPT_VERSION" --disable-doc --with-libgpg-error-prefix="$out"

    # OpenSSL: the one FFmpeg is built with, so the app carries one.
    "$here/../ffmpeg/build.sh" mac
    ssl=$(cd "$here/../ffmpeg/out/mac" && pwd)
    export PKG_CONFIG_PATH="${PKG_CONFIG_PATH:+$PKG_CONFIG_PATH:}$ssl/lib/pkgconfig"
    fetch "glib-networking-$GLIB_NETWORKING_VERSION" "https://download.gnome.org/sources/glib-networking/${GLIB_NETWORKING_VERSION%.*}/glib-networking-$GLIB_NETWORKING_VERSION.tar.xz"
    meson_lib "glib-networking-$GLIB_NETWORKING_VERSION" -Dgnutls=disabled -Dopenssl=enabled -Dlibproxy=disabled -Dgnome_proxy=disabled -Dtests=false
    # The module finds OpenSSL two folders up, where WPE's libraries are and where it is copied.
    cp "$ssl/lib/libssl.3.dylib" "$ssl/lib/libcrypto.3.dylib" "$out/lib/"
    module=$out/lib/gio/modules/libgioopenssl.so
    otool -l "$module" | grep -q '@loader_path/../..' || install_name_tool -add_rpath @loader_path/../.. "$module"
    # GIO's TLS is a module, looked for where GLib was installed unless told: here, and in the
    # app (SpatiMesh's engine program sets it the same way).
    export GIO_EXTRA_MODULES="$out/lib/gio/modules"
    fetch "libsoup-$LIBSOUP_VERSION" "https://download.gnome.org/sources/libsoup/${LIBSOUP_VERSION%.*}/libsoup-$LIBSOUP_VERSION.tar.xz"
    meson_lib "libsoup-$LIBSOUP_VERSION" -Dtests=false -Dintrospection=disabled -Dvapi=disabled -Ddocs=disabled \
        -Dgssapi=disabled -Dntlm=disabled -Dbrotli=enabled -Dsysprof=disabled -Dpkcs11_tests=disabled -Dautobahn=disabled
    # ICU is the system's (libicucore, as Apple's WebKit uses), with the headers WebKit carries
    # for it: nothing to ship. Said to pkg-config so HarfBuzz uses the same one.
    mkdir -p "$out/lib/pkgconfig"
    for icu in icu-uc icu-i18n; do
        cat > "$out/lib/pkgconfig/$icu.pc" <<PC
Name: $icu
Description: the system's ICU (libicucore), WebKit's headers for it
Version: 74.0
Cflags: -I$webkit/Source/WTF/icu -DU_DISABLE_RENAMING=1
Libs: -licucore
PC
    done
    # Text shaping, with the ICU part WebKit asks for (Homebrew's has not got it).
    fetch "harfbuzz-$HARFBUZZ_VERSION" "https://github.com/harfbuzz/harfbuzz/releases/download/$HARFBUZZ_VERSION/harfbuzz-$HARFBUZZ_VERSION.tar.xz"
    meson_lib "harfbuzz-$HARFBUZZ_VERSION" -Dicu=enabled -Dfreetype=enabled -Dglib=enabled -Dgobject=disabled \
        -Dcairo=disabled -Dchafa=disabled -Dtests=disabled -Ddocs=disabled -Dintrospection=disabled -Dutilities=disabled -Dcoretext=disabled
    # GL for Skia and WPE: libepoxy with EGL, which on a Mac is ANGLE's (patched to load it):
    # the compositor's own ANGLE, beside WPE's libraries as it is in the app.
    [ -f "$here/../../mac/vendor/angle/libEGL.dylib" ] || "$here/../../mac/fetch-angle.sh"
    mkdir -p "$out/lib"
    cp "$here/../../mac/vendor/angle/libEGL.dylib" "$here/../../mac/vendor/angle/libGLESv2.dylib" "$out/lib/"
    mkdir -p "$out/lib/pkgconfig"
    # libepoxy loads it at run time; GStreamer's GL links it (@rpath, as in the app).
    cat > "$out/lib/pkgconfig/egl.pc" <<PC
Name: egl
Description: ANGLE's EGL
Version: 1.5
Cflags: -I$webkit/Source/ThirdParty/ANGLE/include
Libs: -L$out/lib -lEGL
PC
    cat > "$out/lib/pkgconfig/glesv2.pc" <<PC
Name: glesv2
Description: ANGLE's OpenGL ES
Version: 3.0
Cflags: -I$webkit/Source/ThirdParty/ANGLE/include
Libs: -L$out/lib -lGLESv2
PC
    fetch "libepoxy-$LIBEPOXY_VERSION" "https://github.com/anholt/libepoxy/archive/refs/tags/$LIBEPOXY_VERSION.tar.gz"
    meson_lib "libepoxy-$LIBEPOXY_VERSION" -Degl=yes -Dglx=no -Dx11=false -Dtests=false -Ddocs=false
    # epoxy/egl.h includes EGL's own headers: ANGLE's, beside it; GStreamer's GL includes GLES's.
    for headers in EGL KHR GLES2 GLES3; do cp -R "$webkit/Source/ThirdParty/ANGLE/include/$headers" "$out/include/"; done
    # The keyboard: macOS has no xkbcommon. Its grammar needs a newer bison than macOS's 2.3.
    fetch "libxkbcommon-$XKBCOMMON_VERSION" "https://xkbcommon.org/download/libxkbcommon-$XKBCOMMON_VERSION.tar.xz"
    PATH=/usr/local/opt/bison/bin:/opt/homebrew/opt/bison/bin:$PATH meson_lib "libxkbcommon-$XKBCOMMON_VERSION" \
        -Denable-x11=false -Denable-wayland=false -Denable-docs=false -Denable-tools=false -Denable-xkbregistry=false
    # Media: WPE's own GStreamer player (video, Media Source Extensions, WebAudio), with only
    # the plugins pages need. Decoding is gst-libav on Spatiand's FFmpeg (third_party/ffmpeg, in
    # PKG_CONFIG_PATH above), so the app carries one copy of the codecs; VideoToolbox decodes in
    # hardware (applemedia). GStreamer's GL is macOS's own (CGL), which applemedia needs; WebKit
    # does not draw video through it, but takes VideoToolbox's frames as IOSurfaces into ANGLE.
    gst=https://gstreamer.freedesktop.org/src
    gst_off="-Dauto_features=disabled -Dtests=disabled -Dexamples=disabled -Ddoc=disabled -Dnls=disabled -Dintrospection=disabled"
    fetch "gstreamer-$GSTREAMER_VERSION" "$gst/gstreamer/gstreamer-$GSTREAMER_VERSION.tar.xz"
    meson_lib "gstreamer-$GSTREAMER_VERSION" $gst_off -Dtools=enabled -Dcheck=enabled -Dcoretracers=disabled
    fetch "gst-plugins-base-$GSTREAMER_VERSION" "$gst/gst-plugins-base/gst-plugins-base-$GSTREAMER_VERSION.tar.xz"
    meson_lib "gst-plugins-base-$GSTREAMER_VERSION" $gst_off -Dtools=disabled \
        -Dapp=enabled -Daudioconvert=enabled -Daudioresample=enabled -Dplayback=enabled -Dtypefind=enabled \
        -Dvideoconvertscale=enabled -Dvolume=enabled -Dsubparse=enabled -Dgio=enabled -Doverlaycomposition=enabled -Drawparse=enabled \
        -Dgl=enabled -Dgl_api=opengl -Dgl_platform=cgl -Dgl_winsys=cocoa
    fetch "gst-plugins-good-$GSTREAMER_VERSION" "$gst/gst-plugins-good/gst-plugins-good-$GSTREAMER_VERSION.tar.xz"
    meson_lib "gst-plugins-good-$GSTREAMER_VERSION" ${gst_off% -Dintrospection=disabled} \
        -Disomp4=enabled -Dmatroska=enabled -Daudioparsers=enabled -Dautodetect=enabled -Dosxaudio=enabled \
        -Dwavparse=enabled -Did3demux=enabled -Dapetag=enabled -Dicydemux=enabled \
        -Ddeinterlace=enabled -Dvideofilter=enabled -Daudiofx=enabled
    fetch "gst-plugins-bad-$GSTREAMER_VERSION" "$gst/gst-plugins-bad/gst-plugins-bad-$GSTREAMER_VERSION.tar.xz"
    meson_lib "gst-plugins-bad-$GSTREAMER_VERSION" $gst_off -Dtools=disabled \
        -Dapplemedia=enabled -Dgl=enabled -Dvideoparsers=enabled -Dsubenc=enabled -Dmpegtsdemux=enabled -Dcodecalpha=enabled
    fetch "gst-libav-$GSTREAMER_VERSION" "$gst/gst-libav/gst-libav-$GSTREAMER_VERSION.tar.xz"
    meson_lib "gst-libav-$GSTREAMER_VERSION" -Dtests=disabled -Ddoc=disabled
    # It finds FFmpeg where FFmpeg's build put it, and in the app in Contents/Frameworks, three
    # folders up from the plugin (Frameworks/wpe/lib/gstreamer-1.0).
    libav="$out/lib/gstreamer-1.0/libgstlibav.dylib"
    for path in "$ssl/lib" @loader_path/../../..; do
        otool -l "$libav" | grep -q "path $path " || install_name_tool -add_rpath "$path" "$libav"
    done
    codesign -f -s - "$libav"
    # Pages' sound into the Spatiand room, at SpatiMesh's window, through its HRTF (gst/).
    sink="$out/lib/gstreamer-1.0/libgstspatiand.dylib"
    if [ ! -f "$sink" ] || [ "$here/gst/spatiandaudiosink.c" -nt "$sink" ]; then
        cc -dynamiclib -O2 -o "$sink" "$here/gst/spatiandaudiosink.c" $(pkg-config --cflags --libs gstreamer-audio-1.0) \
            -install_name "$out/lib/gstreamer-1.0/libgstspatiand.dylib" -Wl,-rpath,"$out/lib"
        codesign -f -s - "$sink"
    fi
    [ -n "${DEPS_ONLY:-}" ] && { echo "libraries for $platform in $out (DEPS_ONLY: WebKit not built)"; exit 0; }
    sdk=$(xcrun --show-sdk-path)
    platform_options="-DCMAKE_OSX_SYSROOT=$sdk -DCMAKE_OSX_DEPLOYMENT_TARGET=14.0
        -DEpoxy_INCLUDE_DIR=$out/include -DEpoxy_LIBRARY=$out/lib/libepoxy.dylib"
    ;;
linux)
    # GCC 12 at least, as WebKit wants: on 22.04 beside its own 11.
    if [ -z "${CC:-}" ] && [ "$(gcc -dumpversion | cut -d. -f1)" -lt 12 ]; then
        export CC=gcc-12 CXX=g++-12
    fi
    # FFmpeg, for gst-libav: Spatiand's own (third_party/ffmpeg), one copy of the codecs.
    "$here/../ffmpeg/build.sh" linux
    ffmpeg=$(cd "$here/../ffmpeg/out/linux" && pwd)
    if [ -n "${SYSTEM_DEPS:-}" ]; then
        # The distro's libraries (Ubuntu 24.04's are new enough): for building and testing WPE
        # and SpatiMesh on it, not for shipping.
        for m in glib-2.0 libsoup-3.0 harfbuzz-icu icu-uc libxml-2.0 sqlite3 epoxy xkbcommon libwebp libjpeg libpng \
            freetype2 fontconfig libwoff2dec libtasn1 libgcrypt gstreamer-1.0 gstreamer-plugins-base-1.0; do
            pkg-config --exists "$m" || { echo "SYSTEM_DEPS: $m missing (its -dev package)" >&2; exit 1; }
        done
        gst_cflags=$(pkg-config --cflags --libs gstreamer-audio-1.0)
        platform_options=
    else
        export PKG_CONFIG_PATH="$out/lib/pkgconfig:$ffmpeg/lib/pkgconfig"
        export CPPFLAGS="-I$out/include" LDFLAGS="-L$out/lib -Wl,-rpath-link,$out/lib"
        export CFLAGS="$CPPFLAGS" CXXFLAGS="$CPPFLAGS"
        export LD_LIBRARY_PATH="$out/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
        gnu=https://ftp.gnu.org/gnu
        fetch "zstd-$ZSTD_VERSION" "https://github.com/facebook/zstd/releases/download/v$ZSTD_VERSION/zstd-$ZSTD_VERSION.tar.gz"
        [ -f "$build/zstd-$ZSTD_VERSION/.installed" ] || { cmake -S "$src/zstd-$ZSTD_VERSION/build/cmake" -B "$build/zstd-$ZSTD_VERSION" -G Ninja \
            -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX="$out" -DCMAKE_INSTALL_LIBDIR=lib \
            -DZSTD_BUILD_PROGRAMS=OFF -DZSTD_BUILD_STATIC=OFF -DZSTD_BUILD_TESTS=OFF && ninja -C "$build/zstd-$ZSTD_VERSION" install && touch "$build/zstd-$ZSTD_VERSION/.installed"; }
        fetch "brotli-$BROTLI_VERSION" "https://github.com/google/brotli/archive/refs/tags/v$BROTLI_VERSION.tar.gz"
        cmake_lib "brotli-$BROTLI_VERSION" -DBROTLI_DISABLE_TESTS=ON -DBROTLI_BUILD_TOOLS=OFF
        fetch "woff2-$WOFF2_VERSION" "https://github.com/google/woff2/archive/refs/tags/v$WOFF2_VERSION.tar.gz"
        cmake_lib "woff2-$WOFF2_VERSION" -DCMAKE_POLICY_VERSION_MINIMUM=3.5
        fetch "pcre2-$PCRE2_VERSION" "https://github.com/PCRE2Project/pcre2/releases/download/pcre2-$PCRE2_VERSION/pcre2-$PCRE2_VERSION.tar.bz2"
        cmake_lib "pcre2-$PCRE2_VERSION" -DPCRE2_BUILD_PCRE2GREP=OFF -DPCRE2_BUILD_TESTS=OFF -DPCRE2_SUPPORT_JIT=ON
        # ICU: the distros' differ in version, and so in their libraries' names (libicuuc.so.70).
        # Its data in the library, as the distros build it.
        icu=icu-$ICU_VERSION
        fetch "$icu" "https://github.com/unicode-org/icu/releases/download/release-$(echo "$ICU_VERSION" | tr . -)/icu4c-$(echo "$ICU_VERSION" | tr . _)-src.tgz"
        [ -d "$src/icu" ] && [ ! -d "$src/$icu" ] && mv "$src/icu" "$src/$icu"
        [ -f "$build/$icu/.installed" ] || { mkdir -p "$build/$icu" && (cd "$build/$icu" \
            && "$src/$icu/source/configure" --prefix="$out" --disable-static --enable-shared --disable-tests --disable-samples \
                --disable-extras --disable-icuio --disable-layoutex && make -j"$jobs" && make install) && touch "$build/$icu/.installed"; }
        fetch "libxml2-$LIBXML2_VERSION" "https://download.gnome.org/sources/libxml2/${LIBXML2_VERSION%.*}/libxml2-$LIBXML2_VERSION.tar.xz"
        cmake_lib "libxml2-$LIBXML2_VERSION" -DLIBXML2_WITH_PYTHON=OFF -DLIBXML2_WITH_LZMA=OFF -DLIBXML2_WITH_ICU=OFF \
            -DLIBXML2_WITH_TESTS=OFF -DLIBXML2_WITH_PROGRAMS=OFF
        sqlite=sqlite-autoconf-$SQLITE_VERSION
        fetch "$sqlite" "https://sqlite.org/$SQLITE_YEAR/$sqlite.tar.gz"
        auto_lib "$sqlite" --disable-readline --enable-threadsafe
        fetch "glib-$GLIB_VERSION" "https://download.gnome.org/sources/glib/${GLIB_VERSION%.*}/glib-$GLIB_VERSION.tar.xz"
        meson_lib "glib-$GLIB_VERSION" -Dnls=disabled -Dtests=false -Dintrospection=disabled -Ddocumentation=false -Dman-pages=disabled \
            -Dsysprof=disabled -Dlibelf=disabled -Dglib_debug=disabled -Dselinux=disabled -Dlibmount=disabled -Dxattr=false
        fetch "libpng-$LIBPNG_VERSION" "https://download.sourceforge.net/libpng/libpng-$LIBPNG_VERSION.tar.xz"
        cmake_lib "libpng-$LIBPNG_VERSION" -DPNG_TESTS=OFF -DPNG_TOOLS=OFF -DPNG_STATIC=OFF
        fetch "freetype-$FREETYPE_VERSION" "https://download.savannah.gnu.org/releases/freetype/freetype-$FREETYPE_VERSION.tar.xz"
        meson_lib "freetype-$FREETYPE_VERSION" -Dharfbuzz=disabled -Dbrotli=disabled -Dpng=enabled -Dzlib=system -Dbzip2=enabled -Dtests=disabled
        # The system's fonts and their configuration (/etc/fonts), which the engine host points
        # fontconfig at (FONTCONFIG_FILE): the bundle's own is only a default.
        fetch "fontconfig-$FONTCONFIG_VERSION" "https://gitlab.freedesktop.org/api/v4/projects/890/packages/generic/fontconfig/$FONTCONFIG_VERSION/fontconfig-$FONTCONFIG_VERSION.tar.xz"
        meson_lib "fontconfig-$FONTCONFIG_VERSION" -Ddoc=disabled -Dtests=disabled -Dtools=disabled -Dcache-build=disabled -Dnls=disabled
        fetch "libjpeg-turbo-$LIBJPEG_TURBO_VERSION" "https://github.com/libjpeg-turbo/libjpeg-turbo/releases/download/$LIBJPEG_TURBO_VERSION/libjpeg-turbo-$LIBJPEG_TURBO_VERSION.tar.gz"
        cmake_lib "libjpeg-turbo-$LIBJPEG_TURBO_VERSION" -DENABLE_STATIC=OFF -DWITH_TURBOJPEG=OFF
        fetch "libwebp-$LIBWEBP_VERSION" "https://storage.googleapis.com/downloads.webmproject.org/releases/webp/libwebp-$LIBWEBP_VERSION.tar.gz"
        cmake_lib "libwebp-$LIBWEBP_VERSION" -DWEBP_BUILD_ANIM_UTILS=OFF -DWEBP_BUILD_CWEBP=OFF -DWEBP_BUILD_DWEBP=OFF -DWEBP_BUILD_GIF2WEBP=OFF \
            -DWEBP_BUILD_IMG2WEBP=OFF -DWEBP_BUILD_VWEBP=OFF -DWEBP_BUILD_WEBPINFO=OFF -DWEBP_BUILD_WEBPMUX=OFF -DWEBP_BUILD_EXTRAS=OFF
        fetch "nghttp2-$NGHTTP2_VERSION" "https://github.com/nghttp2/nghttp2/releases/download/v$NGHTTP2_VERSION/nghttp2-$NGHTTP2_VERSION.tar.xz"
        cmake_lib "nghttp2-$NGHTTP2_VERSION" -DENABLE_LIB_ONLY=ON -DBUILD_STATIC_LIBS=OFF -DENABLE_DOC=OFF -DBUILD_TESTING=OFF
        fetch "libpsl-$LIBPSL_VERSION" "https://github.com/rockdaboot/libpsl/releases/download/$LIBPSL_VERSION/libpsl-$LIBPSL_VERSION.tar.gz"
        meson_lib "libpsl-$LIBPSL_VERSION" -Druntime=no -Dtests=false -Ddocs=false
        fetch "libtasn1-$LIBTASN1_VERSION" "$gnu/libtasn1/libtasn1-$LIBTASN1_VERSION.tar.gz"
        auto_lib "libtasn1-$LIBTASN1_VERSION" --disable-doc
        fetch "libgpg-error-$LIBGPG_ERROR_VERSION" "https://gnupg.org/ftp/gcrypt/libgpg-error/libgpg-error-$LIBGPG_ERROR_VERSION.tar.bz2"
        auto_lib "libgpg-error-$LIBGPG_ERROR_VERSION" --disable-doc --disable-tests --disable-nls
        fetch "libgcrypt-$LIBGCRYPT_VERSION" "https://gnupg.org/ftp/gcrypt/libgcrypt/libgcrypt-$LIBGCRYPT_VERSION.tar.bz2"
        auto_lib "libgcrypt-$LIBGCRYPT_VERSION" --disable-doc --with-libgpg-error-prefix="$out"
        # TLS on the system's OpenSSL 3, as FFmpeg's (every distro Spatiand runs on has it, and
        # keeps it patched).
        fetch "glib-networking-$GLIB_NETWORKING_VERSION" "https://download.gnome.org/sources/glib-networking/${GLIB_NETWORKING_VERSION%.*}/glib-networking-$GLIB_NETWORKING_VERSION.tar.xz"
        meson_lib "glib-networking-$GLIB_NETWORKING_VERSION" -Dgnutls=disabled -Dopenssl=enabled -Dlibproxy=disabled -Dgnome_proxy=disabled -Dtests=false
        export GIO_EXTRA_MODULES="$out/lib/gio/modules"
        fetch "libsoup-$LIBSOUP_VERSION" "https://download.gnome.org/sources/libsoup/${LIBSOUP_VERSION%.*}/libsoup-$LIBSOUP_VERSION.tar.xz"
        meson_lib "libsoup-$LIBSOUP_VERSION" -Dtests=false -Dintrospection=disabled -Dvapi=disabled -Ddocs=disabled \
            -Dgssapi=disabled -Dntlm=disabled -Dbrotli=enabled -Dsysprof=disabled -Dpkcs11_tests=disabled -Dautobahn=disabled
        fetch "harfbuzz-$HARFBUZZ_VERSION" "https://github.com/harfbuzz/harfbuzz/releases/download/$HARFBUZZ_VERSION/harfbuzz-$HARFBUZZ_VERSION.tar.xz"
        meson_lib "harfbuzz-$HARFBUZZ_VERSION" -Dicu=enabled -Dfreetype=enabled -Dglib=enabled -Dgobject=disabled \
            -Dcairo=disabled -Dchafa=disabled -Dtests=disabled -Ddocs=disabled -Dintrospection=disabled -Dutilities=disabled
        # GL: libepoxy, which loads the system's EGL and GLES (Mesa's) when WPE starts.
        fetch "libepoxy-$LIBEPOXY_VERSION" "https://github.com/anholt/libepoxy/archive/refs/tags/$LIBEPOXY_VERSION.tar.gz"
        meson_lib "libepoxy-$LIBEPOXY_VERSION" -Degl=yes -Dglx=no -Dx11=false -Dtests=false -Ddocs=false
        # The keyboard's layouts are the system's (every desktop has them, at this path).
        fetch "libxkbcommon-$XKBCOMMON_VERSION" "https://xkbcommon.org/download/libxkbcommon-$XKBCOMMON_VERSION.tar.xz"
        meson_lib "libxkbcommon-$XKBCOMMON_VERSION" -Denable-x11=false -Denable-wayland=false -Denable-docs=false -Denable-tools=false \
            -Denable-xkbregistry=false -Dxkb-config-root=/usr/share/X11/xkb
        # Media, as on the Mac: GStreamer with only what pages need, decoding by gst-libav on
        # Spatiand's FFmpeg, in hardware by VA-API (va); sound through PulseAudio's client
        # (PipeWire's, on SteamOS) where Spatiand's own sink is not used. No GStreamer GL.
        gst=https://gstreamer.freedesktop.org/src
        gst_off="-Dauto_features=disabled -Dtests=disabled -Dexamples=disabled -Ddoc=disabled -Dnls=disabled -Dintrospection=disabled"
        fetch "gstreamer-$GSTREAMER_VERSION" "$gst/gstreamer/gstreamer-$GSTREAMER_VERSION.tar.xz"
        meson_lib "gstreamer-$GSTREAMER_VERSION" $gst_off -Dtools=enabled -Dcheck=enabled -Dcoretracers=disabled
        fetch "gst-plugins-base-$GSTREAMER_VERSION" "$gst/gst-plugins-base/gst-plugins-base-$GSTREAMER_VERSION.tar.xz"
        meson_lib "gst-plugins-base-$GSTREAMER_VERSION" $gst_off -Dtools=disabled \
            -Dapp=enabled -Daudioconvert=enabled -Daudioresample=enabled -Dplayback=enabled -Dtypefind=enabled \
            -Dvideoconvertscale=enabled -Dvolume=enabled -Dsubparse=enabled -Dgio=enabled -Doverlaycomposition=enabled -Drawparse=enabled -Dgl=disabled
        fetch "gst-plugins-good-$GSTREAMER_VERSION" "$gst/gst-plugins-good/gst-plugins-good-$GSTREAMER_VERSION.tar.xz"
        meson_lib "gst-plugins-good-$GSTREAMER_VERSION" ${gst_off% -Dintrospection=disabled} \
            -Disomp4=enabled -Dmatroska=enabled -Daudioparsers=enabled -Dautodetect=enabled -Dpulse=enabled \
            -Dwavparse=enabled -Did3demux=enabled -Dapetag=enabled -Dicydemux=enabled \
            -Ddeinterlace=enabled -Dvideofilter=enabled -Daudiofx=enabled
        fetch "gst-plugins-bad-$GSTREAMER_VERSION" "$gst/gst-plugins-bad/gst-plugins-bad-$GSTREAMER_VERSION.tar.xz"
        meson_lib "gst-plugins-bad-$GSTREAMER_VERSION" $gst_off -Dtools=disabled \
            -Dva=enabled -Dvideoparsers=enabled -Dsubenc=enabled -Dmpegtsdemux=enabled -Dcodecalpha=enabled
        fetch "gst-libav-$GSTREAMER_VERSION" "$gst/gst-libav/gst-libav-$GSTREAMER_VERSION.tar.xz"
        meson_lib "gst-libav-$GSTREAMER_VERSION" -Dtests=disabled -Ddoc=disabled
        # FFmpeg in the bundle, beside the rest.
        cp -P "$ffmpeg"/lib/lib*.so* "$out/lib/"
        gst_cflags="-I$out/include $(pkg-config --cflags --libs gstreamer-audio-1.0) -Wl,-rpath,\$ORIGIN/../.."
        # Every program and library of WebKit's finds the rest by where it is.
        platform_options="-DCMAKE_INSTALL_RPATH=\$ORIGIN:\$ORIGIN/../lib:\$ORIGIN/../../lib -DCMAKE_BUILD_WITH_INSTALL_RPATH=OFF
            -DCMAKE_SKIP_INSTALL_RPATH=OFF"
    fi
    # Pages' sound into the Spatiand room, at SpatiMesh's window, through its HRTF (gst/).
    sink="$out/lib/gstreamer-1.0/libgstspatiand.so"
    mkdir -p "$out/lib/gstreamer-1.0"
    if [ ! -f "$sink" ] || [ "$here/gst/spatiandaudiosink.c" -nt "$sink" ]; then
        ${CC:-cc} -shared -fPIC -O2 -o "$sink" "$here/gst/spatiandaudiosink.c" $gst_cflags
    fi
    [ -n "${DEPS_ONLY:-}" ] && { echo "libraries for $platform in $out (DEPS_ONLY: WebKit not built)"; exit 0; }
    ;;
*) echo "no such platform yet: $platform" >&2; exit 1 ;;
esac

# WebKit, the WPE port, with only what SpatiMesh draws through: the headless platform (pictures
# handed to Spatiand), no Wayland or DRM, no GObject introspection or documentation. Media is
# WPE's GStreamer player (above), without GStreamer's GL; the inspector is on (developer tools). WebGL is WebKit's
# own ANGLE, which on a Mac draws through the ANGLE the rest of WPE uses (as on Linux it draws
# through Mesa); web fonts in WOFF2 are read with Google's woff2.
# Spatiand's private hand-over of pictures (crates/spatiand-surfaces), which WPE on a Mac builds
# in under names of its own (Source/WebKit/WPEPlatform/wpe/WPESurfaces.h): copied in, not
# patched, so there is one source of it.
if [ "$platform" = mac ]; then
    mkdir -p "$webkit/Source/WebKit/WPEPlatform/wpe/spatiand"
    for f in surfaces.c surfaces.h; do
        cmp -s "$here/../../crates/spatiand-surfaces/native/$f" "$webkit/Source/WebKit/WPEPlatform/wpe/spatiand/$f" \
            || cp "$here/../../crates/spatiand-surfaces/native/$f" "$webkit/Source/WebKit/WPEPlatform/wpe/spatiand/$f"
    done
fi
mkdir -p "$build/webkit"
(cd "$build/webkit" && cmake -G Ninja "$webkit" -DPORT=WPE -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX="$out" \
    -DCMAKE_PREFIX_PATH="$out" -DCMAKE_IGNORE_PREFIX_PATH="/usr/local;/opt/homebrew" \
    $platform_options \
    -DENABLE_WPE_PLATFORM=ON -DENABLE_WPE_PLATFORM_HEADLESS=ON -DENABLE_WPE_PLATFORM_DRM=OFF -DENABLE_WPE_PLATFORM_WAYLAND=OFF \
    -DENABLE_WPE_LEGACY_API=OFF -DENABLE_WPE_QT_API=OFF -DUSE_GBM=OFF -DUSE_LIBDRM=OFF \
    -DENABLE_INTROSPECTION=OFF -DENABLE_DOCUMENTATION=OFF -DENABLE_JOURNALD_LOG=OFF -DUSE_ATK=OFF -DUSE_FLITE=OFF \
    -DUSE_LIBBACKTRACE=OFF -DUSE_LIBHYPHEN=OFF -DUSE_SYSPROF_CAPTURE=OFF -DENABLE_BUBBLEWRAP_SANDBOX=OFF \
    -DENABLE_SPEECH_SYNTHESIS=OFF -DENABLE_GAMEPAD=OFF -DUSE_JPEGXL=OFF -DUSE_AVIF=OFF -DUSE_WOFF2=ON -DUSE_LCMS=OFF \
    -DENABLE_WEBDRIVER=OFF -DENABLE_MINIBROWSER=OFF -DENABLE_API_TESTS=OFF -DENABLE_VIDEO=ON -DENABLE_WEB_AUDIO=ON \
    -DUSE_GSTREAMER=ON -DUSE_GSTREAMER_GL=OFF -DUSE_GSTREAMER_MPEGTS=OFF -DUSE_GSTREAMER_TRANSCODER=OFF \
    -DENABLE_MEDIA_SOURCE=ON -DENABLE_ENCRYPTED_MEDIA=OFF -DENABLE_THUNDER=OFF -DUSE_EXTERNAL_HOLEPUNCH=OFF \
    -DENABLE_MEDIA_RECORDER=OFF -DENABLE_MEDIA_STREAM=OFF -DENABLE_WEB_RTC=OFF -DENABLE_SPELLCHECK=OFF -DENABLE_XSLT=OFF -DENABLE_WEBGL=ON)
ninja -C "$build/webkit" -j"$jobs"
ninja -C "$build/webkit" install
if [ "$platform" = mac ]; then
    # The web and network processes find WPE's library two folders up (and are signed again,
    # as macOS wants of anything changed).
    for b in "$out"/libexec/wpe-webkit-2.0/WPE*Process; do
        otool -l "$b" | grep -q '@loader_path/../../lib' || install_name_tool -add_rpath @loader_path/../../lib "$b"
        codesign -f -s - "$b"
    done
fi
if [ "$platform" = linux ] && [ -z "${SYSTEM_DEPS:-}" ]; then
    # The bundle's libraries and plugins find each other by where they are, wherever it is put
    # (meson and autotools leave the build's own paths, or none).
    for f in "$out"/lib/lib*.so*; do
        [ -L "$f" ] || patchelf --set-rpath '$ORIGIN' "$f"
    done
    for f in "$out"/lib/gstreamer-1.0/*.so "$out"/lib/gio/modules/*.so "$out"/lib/wpe-webkit-2.0/*/*.so; do
        [ -f "$f" ] && patchelf --set-rpath '$ORIGIN/../..:$ORIGIN/..' "$f"
    done
    for f in "$out"/libexec/*/* ; do
        [ -f "$f" ] && [ -x "$f" ] && file -b "$f" | grep -q ELF && patchelf --set-rpath '$ORIGIN/../../lib' "$f"
    done
fi
echo "WPE $WPE_VERSION for $platform in $out"
