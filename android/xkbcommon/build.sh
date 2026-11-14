#!/bin/sh
# libxkbcommon for Android, and the keyboard layouts it reads: what the compositor's keyboard
# is made of (smithay links it), which Android does not have.
#
#   android/xkbcommon/build.sh      -> out/libxkbcommon.a and out/xkb/ (the layouts), and
#                                      out/libxkbcommon.so with out/include: SpatiMesh's Chromium's
#                                      (third_party/chromium/build.sh android), which the APK carries;
#                                      out/VERSION, the library's version, for its pkg-config file
#
# What is built, its version and its layouts are third_party/xkbcommon/source.sh's, the Mac's
# too (mac/xkbcommon/build.sh); here only bionic's side of config.h and the NDK's clang, used
# directly rather than meson. Run by android/build.sh; everything lands in out/, not committed.
set -eu
cd "$(dirname "$0")"
. ../../third_party/xkbcommon/source.sh
. ../ndk.sh
NDK=$SPATIAND_NDK
API=34
# The NDK's tools are in a folder named for the machine building: a Mac's or Linux's.
HOST_TAG=$(case "$(uname -s)" in (Darwin) echo darwin-x86_64 ;; (*) echo linux-x86_64 ;; esac)

# Built once, and again with another NDK (ndk.sh) or another release (source.sh).
[ -f out/libxkbcommon.a ] && [ -f out/libxkbcommon.so ] && [ -f out/xkb/rules/evdev ] &&
    [ "$(cat out/.ndk 2>/dev/null)" = "$SPATIAND_NDK_REVISION $XKB_VERSION $XKB_DATA" ] && exit 0

rm -rf work out && mkdir -p work out
xkb_prepare work
# What meson would find out about bionic at API 34 (it cannot run what it compiles for the
# phone, so it leaves out newlocale, as for any C library that is not glibc): no eaccess, no
# secure_getenv.
cat >> work/config.h <<'EOF'
#define _GNU_SOURCE 1
EOF

CC="$NDK/toolchains/llvm/prebuilt/$HOST_TAG/bin/aarch64-linux-android$API-clang"
AR="$NDK/toolchains/llvm/prebuilt/$HOST_TAG/bin/llvm-ar"
OBJECTS=""
mkdir -p work/obj
for s in $XKB_SOURCES work/parser.c; do
    case "$s" in work/*) from="$s" ;; *) from="$XKB_SRC/$s" ;; esac
    o=work/obj/$(echo "$s" | tr / _).o
    # shellcheck disable=SC2086
    "$CC" -c $XKB_CFLAGS -o "$o" "$from"
    OBJECTS="$OBJECTS $o"
done
# shellcheck disable=SC2086
"$AR" rcs out/libxkbcommon.a $OBJECTS
# The same, shared, with the symbol versions meson gives it (xkbcommon.map: V_0.5.0 and on), which
# a program linked against a meson-built one asks for by name. Its soname stays libxkbcommon.so,
# as the APK carries it.
# shellcheck disable=SC2086
"$CC" -shared -Wl,-soname,libxkbcommon.so -Wl,--version-script=$XKB_SRC/xkbcommon.map -o out/libxkbcommon.so $OBJECTS
mkdir -p out/include && cp -R "$XKB_SRC/include/xkbcommon" out/include/
cp -R work/xkb out/xkb
echo "$XKB_VERSION" > out/VERSION
rm -rf work
echo "$SPATIAND_NDK_REVISION $XKB_VERSION $XKB_DATA" > out/.ndk
echo "built $(pwd)/out/libxkbcommon.a, out/libxkbcommon.so and out/xkb ($XKB_VERSION, xkb-data $XKB_DATA)"
