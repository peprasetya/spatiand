#!/bin/bash
# What Chromium's Linux code takes from the system that Android's libc has not got, built for it
# with the NDK, as termux-packages builds them (packages/libnspr, libnss, dbus):
#
#   NSPR and NSS -- certificates as on Linux: client certificates, the certificate manager, and
#                   Firefox's saved passwords. Shared, as NSS loads its own modules by name; every
#                   name is already one Android's library folder takes (libnss3.so).
#   libdbus      -- Chromium's D-Bus client. Android has no bus for it to reach, and it says so;
#                   static, so its versioned name (libdbus-1.so.3) never has to be carried.
#
#   third_party/chromium/android-libc/deps.sh <out folder>
#
# For API 24, as the rest of the build (README.md). Run on Linux with the NDK at SPATIAND_NDK.
set -e

NSPR_VERSION=4.40
NSS_VERSION=3.133
DBUS_VERSION=1.16.2

here=$(cd "$(dirname "$0")" && pwd)
out=$(mkdir -p "$1" && cd "$1" && pwd)
work=$out/.work
mkdir -p "$work" "$out/lib/pkgconfig" "$out/include"
NDK=${SPATIAND_NDK:-/media/shared/Linux/App/android-ndk-r28c}
API=24
T=$NDK/toolchains/llvm/prebuilt/linux-x86_64/bin
TARGET=aarch64-linux-android
export CC=$T/$TARGET$API-clang CXX=$T/$TARGET$API-clang++ AR=$T/llvm-ar RANLIB=$T/llvm-ranlib STRIP=$T/llvm-strip
jobs=$(nproc)

fetch() { # url folder
    [ -d "$work/$2" ] && return
    curl -fsSL "$1" | tar -xz -C "$work"
}

# --- NSPR ---
if [ ! -f "$out/lib/libnspr4.so" ]; then
    fetch "https://archive.mozilla.org/pub/nspr/releases/v$NSPR_VERSION/src/nspr-$NSPR_VERSION.tar.gz" "nspr-$NSPR_VERSION"
    # Configured as for Linux, with the NDK's compiler given: its own Android setup wants the NDK's
    # old layout (Termux's packages/libnspr/configure.patch).
    grep -q 'case "" in' "$work/nspr-$NSPR_VERSION/nspr/configure" ||
        (cd "$work/nspr-$NSPR_VERSION/nspr" && patch -p1 -s < "$here/deps-patches/nspr-configure.patch")
    mkdir -p "$work/nspr-build" && cd "$work/nspr-build"
    CFLAGS="-DANDROID -O2" LDFLAGS="-llog" HOST_CC=gcc HOST_CFLAGS="-O2" \
        "$work/nspr-$NSPR_VERSION/nspr/configure" --prefix="$out" --host=$TARGET --target=$TARGET \
        --enable-64bit --enable-optimize --disable-debug
    make -j"$jobs" HOST_CC=gcc
    make install
fi

# --- NSS ---
# Its database is SQLite: the WPE build's for Android, which the APK carries.
SQLITE=${SPATIAND_ANDROID_DEPS:-/media/shared/Linux/App/spatiand-wpe/out/android}
if [ ! -f "$out/lib/libnss3.so" ]; then
    fetch "https://archive.mozilla.org/pub/security/nss/releases/NSS_${NSS_VERSION//./_}_RTM/src/nss-$NSS_VERSION.tar.gz" "nss-$NSS_VERSION"
    src=$work/nss-$NSS_VERSION/nss
    # Termux's packages/libnss patches: shared libraries linked with LDFLAGS, and getenv where
    # glibc's secure_getenv is.
    if [ ! -f "$work/nss-$NSS_VERSION/.patched" ]; then
        for p in "$here"/deps-patches/nss-*.patch; do (cd "$work/nss-$NSS_VERSION" && patch -p1 -s < "$p"); done
        touch "$work/nss-$NSS_VERSION/.patched"
    fi
    # Libraries a Linux build links that Android's libc already is (as for Chromium, build.sh).
    mkdir -p "$work/stubs"
    printf '!<arch>\n' > "$work/stubs/libpthread.a"
    printf '!<arch>\n' > "$work/stubs/librt.a"
    # Its installer, a program of this machine's.
    mkdir -p "$work/nsinstall"
    gcc -O2 -o "$work/nsinstall/nsinstall" "$src/coreconf/nsinstall/nsinstall.c" "$src/coreconf/nsinstall/pathsub.c"
    make -C "$src" -j1 BUILD_OPT=1 USE_64=1 CC_IS_CLANG=1 CROSS_COMPILE=1 OS_TEST=aarch64 \
        NSPR_INCLUDE_DIR="$out/include/nspr" NSPR_LIB_DIR="$out/lib" \
        NSS_DISABLE_GTESTS=1 NSS_ENABLE_WERROR=0 NSS_SEED_ONLY_DEV_URANDOM=1 NSS_USE_SYSTEM_SQLITE=1 \
        NSINSTALL="$work/nsinstall/nsinstall" CC="$CC" CCC="$CXX" \
        XCFLAGS="-DANDROID -O2 -I$SQLITE/include" CPPFLAGS="-DANDROID" LDFLAGS="-L$work/stubs -L$SQLITE/lib -llog"
    dist=$work/nss-$NSS_VERSION/dist
    mkdir -p "$out/include/nss"
    cp -L "$dist"/public/nss/*.h "$out/include/nss/"
    cp -L "$dist"/*.OBJ/lib/*.so "$out/lib/"
fi
cat > "$out/lib/pkgconfig/nspr.pc" <<PC
prefix=/usr
libdir=\${prefix}/lib
includedir=\${prefix}/include/nspr
Name: NSPR
Description: The Netscape Portable Runtime
Version: $NSPR_VERSION
Libs: -L\${libdir} -lplds4 -lplc4 -lnspr4
Cflags: -I\${includedir}
PC
cat > "$out/lib/pkgconfig/nss.pc" <<PC
prefix=/usr
libdir=\${prefix}/lib
includedir=\${prefix}/include/nss
Name: NSS
Description: Network Security Services
Version: $NSS_VERSION
Requires: nspr
Libs: -L\${libdir} -lssl3 -lsmime3 -lnss3 -lnssutil3
Cflags: -I\${includedir}
PC

# --- libdbus ---
if [ ! -f "$out/lib/libdbus-1.a" ]; then
    [ -x "$work/.venv/bin/meson" ] || { python3 -m venv "$work/.venv" && "$work/.venv/bin/pip" install -q meson; }
    [ -d "$work/dbus-$DBUS_VERSION" ] || curl -fsSL "https://dbus.freedesktop.org/releases/dbus/dbus-$DBUS_VERSION.tar.xz" | tar -xJ -C "$work"
    cat > "$work/android.ini" <<INI
[binaries]
c = '$CC'
cpp = '$CXX'
ar = '$AR'
strip = '$STRIP'
pkg-config = 'false'

[built-in options]
c_args = ['-DANDROID']
c_link_args = ['-llog']

[host_machine]
system = 'android'
cpu_family = 'aarch64'
cpu = 'aarch64'
endian = 'little'
INI
    "$work/.venv/bin/meson" setup --wipe "$work/dbus-build" "$work/dbus-$DBUS_VERSION" --cross-file "$work/android.ini" \
        --prefix="$out" --libdir=lib --buildtype=release --default-library=static \
        -Dmessage_bus=false -Dtools=false -Dmodular_tests=disabled -Dsystemd=disabled -Dlibaudit=disabled \
        -Dselinux=disabled -Dapparmor=disabled -Dx11_autolaunch=disabled -Ddoxygen_docs=disabled \
        -Dxml_docs=disabled -Dqt_help=disabled -Dducktype_docs=disabled -Dinotify=disabled \
        -Dsession_socket_dir=/tmp -Dtest_socket_dir=/tmp
    ninja -C "$work/dbus-build" install
    sed -i 's|^prefix=.*|prefix=/usr|' "$out/lib/pkgconfig/dbus-1.pc"
fi
echo "built in $out: $(ls "$out/lib" | tr '\n' ' ')"
