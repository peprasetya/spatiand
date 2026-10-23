#!/bin/sh
# WPE WebKit for SpatiMesh, built from its release with Spatiand's own patches (patches/<name>/,
# numbered, applied in order) -- on the Mac, where WebKit upstream builds only as Apple's own
# WebKit, WPE is ported here.
#
#   third_party/wpe/build.sh mac
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
jobs=$( (sysctl -n hw.ncpu || nproc) 2>/dev/null )
# meson and ninja for the GNOME libraries, from a folder of this script's own.
[ -x "$work/.venv/bin/meson" ] || { python3 -m venv "$work/.venv" && "$work/.venv/bin/pip" install -q meson ninja; }
PATH=$work/.venv/bin:$PATH
export PKG_CONFIG_PATH="$out/lib/pkgconfig${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"

fetch() { # name url
    [ -d "$src/$1" ] && return
    case $2 in *.gz) unpack=-xz ;; *) unpack=-xJ ;; esac
    curl -fsSL "$2" | tar $unpack -C "$src"
    for p in "$here/patches/${1%-*}"/*.patch; do
        [ -f "$p" ] && (cd "$src/$1" && patch -p1 < "$p")
    done
    return 0
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
    for p in "$here"/patches/webkit/*.patch; do
        [ -f "$p" ] && git -C "$webkit" apply "$p"
    done
fi

case $platform in
mac)
    # OpenSSL: the one FFmpeg is built with, so the app carries one.
    "$here/../ffmpeg/build.sh" mac
    ssl=$(cd "$here/../ffmpeg/out/mac" && pwd)
    export PKG_CONFIG_PATH="$PKG_CONFIG_PATH:$ssl/lib/pkgconfig"
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
        -Dgssapi=disabled -Dntlm=disabled -Dbrotli=disabled -Dsysprof=disabled -Dpkcs11_tests=disabled -Dautobahn=disabled
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
    cat > "$out/lib/pkgconfig/egl.pc" <<PC
Name: egl
Description: ANGLE's EGL headers (the library is loaded at run time)
Version: 1.5
Cflags: -I$webkit/Source/ThirdParty/ANGLE/include
PC
    fetch "libepoxy-$LIBEPOXY_VERSION" "https://github.com/anholt/libepoxy/archive/refs/tags/$LIBEPOXY_VERSION.tar.gz"
    meson_lib "libepoxy-$LIBEPOXY_VERSION" -Degl=yes -Dglx=no -Dx11=false -Dtests=false -Ddocs=false
    # epoxy/egl.h includes EGL's own headers: ANGLE's, beside it.
    cp -R "$webkit/Source/ThirdParty/ANGLE/include/EGL" "$webkit/Source/ThirdParty/ANGLE/include/KHR" "$out/include/"
    # The keyboard: macOS has no xkbcommon. Its grammar needs a newer bison than macOS's 2.3.
    fetch "libxkbcommon-$XKBCOMMON_VERSION" "https://xkbcommon.org/download/libxkbcommon-$XKBCOMMON_VERSION.tar.xz"
    PATH=/usr/local/opt/bison/bin:/opt/homebrew/opt/bison/bin:$PATH meson_lib "libxkbcommon-$XKBCOMMON_VERSION" \
        -Denable-x11=false -Denable-wayland=false -Denable-docs=false -Denable-tools=false -Denable-xkbregistry=false
    sdk=$(xcrun --show-sdk-path)
    platform_options="-DCMAKE_OSX_SYSROOT=$sdk -DCMAKE_OSX_DEPLOYMENT_TARGET=14.0
        -DEpoxy_INCLUDE_DIR=$out/include -DEpoxy_LIBRARY=$out/lib/libepoxy.dylib"
    ;;
*) echo "no such platform yet: $platform" >&2; exit 1 ;;
esac

# WebKit, the WPE port, with only what SpatiMesh draws through: the headless platform (pictures
# handed to Spatiand), no Wayland or DRM, no GObject introspection or documentation. Media is off
# until it is SpatiMesh's own (FFmpeg); the inspector is on (developer tools). WebGL is off for
# now: WebKit's own ANGLE would sit on the Mac's ANGLE, which is not done yet.
mkdir -p "$build/webkit"
(cd "$build/webkit" && cmake -G Ninja "$webkit" -DPORT=WPE -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX="$out" \
    $platform_options \
    -DENABLE_WPE_PLATFORM=ON -DENABLE_WPE_PLATFORM_HEADLESS=ON -DENABLE_WPE_PLATFORM_DRM=OFF -DENABLE_WPE_PLATFORM_WAYLAND=OFF \
    -DENABLE_WPE_LEGACY_API=OFF -DENABLE_WPE_QT_API=OFF -DUSE_GBM=OFF -DUSE_LIBDRM=OFF \
    -DENABLE_INTROSPECTION=OFF -DENABLE_DOCUMENTATION=OFF -DENABLE_JOURNALD_LOG=OFF -DUSE_ATK=OFF -DUSE_FLITE=OFF \
    -DUSE_LIBBACKTRACE=OFF -DUSE_LIBHYPHEN=OFF -DUSE_SYSPROF_CAPTURE=OFF -DENABLE_BUBBLEWRAP_SANDBOX=OFF \
    -DENABLE_SPEECH_SYNTHESIS=OFF -DENABLE_GAMEPAD=OFF -DUSE_JPEGXL=OFF -DUSE_AVIF=OFF -DUSE_WOFF2=OFF -DUSE_LCMS=OFF \
    -DENABLE_WEBDRIVER=OFF -DENABLE_MINIBROWSER=OFF -DENABLE_API_TESTS=OFF -DENABLE_VIDEO=OFF -DENABLE_WEB_AUDIO=OFF \
    -DENABLE_MEDIA_STREAM=OFF -DENABLE_WEB_RTC=OFF -DUSE_GSTREAMER=OFF -DENABLE_SPELLCHECK=OFF -DENABLE_XSLT=OFF -DENABLE_WEBGL=OFF)
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
echo "WPE $WPE_VERSION for $platform in $out"
