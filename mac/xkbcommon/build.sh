#!/bin/sh
# libxkbcommon for the Mac, and the keyboard layouts it reads: what the compositor's keyboard
# is made of (smithay links it), which macOS does not have. SpatiMesh links the same library
# (third_party/chromium/mac/ozbuild.sh).
#
#   mac/xkbcommon/build.sh      -> out/libxkbcommon.a (Intel and Apple Silicon) and out/xkb/
#
# What is built, its version and its layouts are third_party/xkbcommon/source.sh's, the Beam
# Pro's too (android/xkbcommon/build.sh); here only macOS's side of config.h and Apple's clang,
# used directly rather than meson. It needs nothing installed: the grammar comes compiled
# (third_party/xkbcommon/generate-parser.sh says why). Run by mac/build.sh and make-app.sh;
# everything lands in out/, which is not committed.
set -eu
cd "$(dirname "$0")"
. ../../third_party/xkbcommon/source.sh

# Built once, and again with another release (source.sh).
[ -f out/libxkbcommon.a ] && [ -f out/xkb/rules/evdev ] &&
    [ "$(cat out/.version 2>/dev/null)" = "$XKB_VERSION $XKB_DATA" ] && exit 0

rm -rf work out && mkdir -p work out
xkb_prepare work
# What meson finds on macOS (checked on littleotter, macOS 27, Apple clang 21): its newlocale
# refuses a locale it has not got, and there is no eaccess, euidaccess or secure_getenv.
# _GNU_SOURCE is meson's own choice everywhere but Solaris; macOS's headers ignore it.
cat >> work/config.h <<'EOF'
#define _GNU_SOURCE 1
#define HAVE_NEWLOCALE 1
EOF

# Once for each kind of Mac, then one library holding both.
for ARCH in x86_64 arm64; do
    CC="clang -arch $ARCH -mmacosx-version-min=13.0"
    OBJECTS=""
    mkdir -p work/obj-$ARCH
    for s in $XKB_SOURCES work/parser.c; do
        case "$s" in work/*) from="$s" ;; *) from="$XKB_SRC/$s" ;; esac
        o=work/obj-$ARCH/$(echo "$s" | tr / _).o
        # shellcheck disable=SC2086
        $CC -c $XKB_CFLAGS -o "$o" "$from"
        OBJECTS="$OBJECTS $o"
    done
    # shellcheck disable=SC2086
    libtool -static -o work/libxkbcommon-$ARCH.a $OBJECTS 2>/dev/null
done
lipo -create work/libxkbcommon-x86_64.a work/libxkbcommon-arm64.a -output out/libxkbcommon.a
mkdir -p out/include && cp -R "$XKB_SRC/include/xkbcommon" out/include/
cp -R work/xkb out/xkb
rm -rf work
echo "$XKB_VERSION $XKB_DATA" > out/.version
echo "built $(pwd)/out/libxkbcommon.a and out/xkb ($XKB_VERSION, xkb-data $XKB_DATA)"
