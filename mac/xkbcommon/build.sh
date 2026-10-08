#!/bin/sh
# libxkbcommon for the Mac, and the keyboard layouts it reads: what the compositor's keyboard
# is made of (smithay links it), which macOS does not have. android/xkbcommon/build.sh is the
# same thing for the Beam Pro, and what it has already fetched is used here.
#
#   mac/xkbcommon/build.sh      -> out/libxkbcommon.a (Intel and Apple Silicon) and out/xkb/
#
# Built with clang directly rather than meson: it is thirty C files and one grammar, and this
# way the Mac needs only bison (Homebrew's; macOS's 2.3 is too old for the grammar).
# The layouts are Debian's xkb-data, taken out of the package, because xkeyboard-config's own
# release needs a Python build step to produce the rules files. Both are pinned by SHA-256.
# Run by android/build.sh; everything lands in out/, which is not committed.
set -eu
cd "$(dirname "$0")"
XKB_VERSION=1.7.0
XKB_SHA=65782f0a10a4b455af9c6baab7040e2f537520caa2ec2092805cdfd36863b247
DATA_DEB=xkb-data_2.42-1_all.deb
DATA_SHA=196ff18533382f64e057ea49df2bb486bd4275a4cc0917361edb560b8756dada
BISON=${BISON:-/usr/local/opt/bison/bin/bison}

[ -f out/libxkbcommon.a ] && [ -f out/xkb/rules/evdev ] && exit 0

fetch() { # url sha file
    mkdir -p cache
    # Already fetched for the Beam Pro's build, in this checkout or the one it was made from.
    for had in ../../android/xkbcommon/cache "$(git rev-parse --path-format=absolute --git-common-dir 2>/dev/null)/../android/xkbcommon/cache"; do
        [ -f "cache/$3" ] || [ ! -f "$had/$3" ] || cp "$had/$3" "cache/$3"
    done
    if [ ! -f "cache/$3" ]; then
        curl -sSfL -o "cache/$3.part" "$1"
        mv "cache/$3.part" "cache/$3"
    fi
    echo "$2  cache/$3" | shasum -a 256 -c - >/dev/null || { echo "$3: checksum mismatch"; exit 1; }
}
fetch "https://xkbcommon.org/download/libxkbcommon-$XKB_VERSION.tar.xz" $XKB_SHA "libxkbcommon-$XKB_VERSION.tar.xz"
fetch "https://deb.debian.org/debian/pool/main/x/xkeyboard-config/$DATA_DEB" $DATA_SHA $DATA_DEB

rm -rf work out && mkdir -p work out
tar -C work -xf "cache/libxkbcommon-$XKB_VERSION.tar.xz"
SRC=work/libxkbcommon-$XKB_VERSION

"$BISON" --defines=work/parser.h -o work/parser.c -p _xkbcommon_ $SRC/src/xkbcomp/parser.y

# What meson would have found out about macOS, and where the layouts live by default. The
# default is never used: the app points XKB_CONFIG_ROOT at its own copy.
cat > work/config.h <<'EOF'
#define _DARWIN_C_SOURCE 1
#define EXIT_INVALID_USAGE 2
#define LIBXKBCOMMON_VERSION "1.7.0"
#define LIBXKBCOMMON_TOOL_PATH ""
#define DFLT_XKB_CONFIG_ROOT "/nonexistent/xkb"
#define DFLT_XKB_CONFIG_EXTRA_PATH "/nonexistent/xkb-extra"
#define XLOCALEDIR "/nonexistent/locale"
#define DEFAULT_XKB_RULES "evdev"
#define DEFAULT_XKB_MODEL "pc105"
#define DEFAULT_XKB_LAYOUT "us"
#define DEFAULT_XKB_VARIANT NULL
#define DEFAULT_XKB_OPTIONS NULL
#define HAVE_UNISTD_H 1
#define HAVE___BUILTIN_EXPECT 1
#define HAVE_MMAP 1
#define HAVE_MKOSTEMP 1
#define HAVE_STRNDUP 1
#define HAVE_ASPRINTF 1
EOF

SOURCES="
src/compose/parser.c src/compose/paths.c src/compose/state.c src/compose/table.c
src/xkbcomp/action.c src/xkbcomp/ast-build.c src/xkbcomp/compat.c src/xkbcomp/expr.c
src/xkbcomp/include.c src/xkbcomp/keycodes.c src/xkbcomp/keymap.c src/xkbcomp/keymap-dump.c
src/xkbcomp/keywords.c src/xkbcomp/rules.c src/xkbcomp/scanner.c src/xkbcomp/symbols.c
src/xkbcomp/types.c src/xkbcomp/vmod.c src/xkbcomp/xkbcomp.c
src/atom.c src/context.c src/context-priv.c src/keysym.c src/keysym-utf.c src/keymap.c
src/keymap-priv.c src/state.c src/text.c src/utf8.c src/utils.c"
# Once for each kind of Mac, then one library holding both.
for ARCH in x86_64 arm64; do
    CC="clang -arch $ARCH -mmacosx-version-min=13.0"
    OBJECTS=""
    mkdir -p work/obj-$ARCH
    for s in $SOURCES work/parser.c; do
        case "$s" in work/*) from="$s" ;; *) from="$SRC/$s" ;; esac
        o=work/obj-$ARCH/$(echo "$s" | tr / _).o
        $CC -c -O2 -fPIC -std=c11 -fvisibility=default -include work/config.h \
            -I work -I $SRC -I $SRC/include -I $SRC/src -I $SRC/src/xkbcomp \
            -w -o "$o" "$from"
        OBJECTS="$OBJECTS $o"
    done
    libtool -static -o work/libxkbcommon-$ARCH.a $OBJECTS 2>/dev/null
done
lipo -create work/libxkbcommon-x86_64.a work/libxkbcommon-arm64.a -output out/libxkbcommon.a

# The layouts: rules, keycodes, types, compat and symbols, as the package installs them.
mkdir -p work/deb && (cd work/deb && ar x "../../cache/$DATA_DEB" && tar -xf data.tar.xz)
cp -R work/deb/usr/share/X11/xkb out/xkb
rm -rf work
echo "built $(pwd)/out/libxkbcommon.a and out/xkb"
