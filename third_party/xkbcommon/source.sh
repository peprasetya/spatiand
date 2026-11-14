# libxkbcommon and the keyboard layouts it reads, as Spatiand builds them where the system has
# none: the Beam Pro (android/xkbcommon/build.sh) and the Mac (mac/xkbcommon/build.sh). What is
# built is said once, here, for both; each of those scripts adds only its own compiler and the
# few things its C library has or has not got (the end of config.h), and links the result its
# own way. The Deck and other Linux use the system's libxkbcommon.
#
# Sourced, not run, from the folder of the script sourcing it:
#   . ../../third_party/xkbcommon/source.sh
#   xkb_prepare work      -> work/libxkbcommon-$XKB_VERSION (the sources), work/parser.[ch],
#                            work/config.h (without its last, platform part), work/xkb (the layouts)
#   $XKB_SOURCES          the library's C files, relative to the sources
#   $XKB_CFLAGS           what every one of them is compiled with (after -include work/config.h)
#
# A new release: change XKB_VERSION and XKB_SHA (the SHA-256 of GitHub's tag archive; the
# release has no other download since xkbcommon.org's stopped at 1.7), compare XKB_SOURCES with
# `libxkbcommon_sources` in its meson.build, compare the config.h below with meson.build's
# `configh_data`, and run generate-parser.sh. New layouts: change XKB_DATA and XKB_DATA_SHA, and
# MainActivity.java's XKB_VERSION, so the phone unpacks them again.

XKB_VERSION=1.13.2   # 2026-05-30
XKB_SHA=acc4d5f7c3cbba5f9f8d08d8bdbeede84ecede46792f47929aa9321873385528
XKB_URL=https://github.com/xkbcommon/libxkbcommon/archive/refs/tags/xkbcommon-$XKB_VERSION.tar.gz
# The layouts are Debian's xkb-data, taken out of the package, because xkeyboard-config's own
# release needs a Python build step to produce the rules files.
XKB_DATA=2.48-1      # xkeyboard-config 2.48, 2026-06-21; Debian's newest (2.49 is not packaged yet)
XKB_DATA_SHA=b3b59c1c304c50739f3a8f9589e08d0e1427a0de98dd132d2607ed7a62d31cb9
XKB_DATA_URL=https://deb.debian.org/debian/pool/main/x/xkeyboard-config/xkb-data_${XKB_DATA}_all.deb

# meson.build's libxkbcommon_sources, without the grammar (parser.y: generate-parser.sh) and the
# headers. Not the X11 library, the tools, nor xkbregistry (registry.c, libxml2), which nothing
# of Spatiand's uses: the room's keyboard and SpatiMesh's call only libxkbcommon itself.
XKB_SOURCES="
src/compose/parser.c src/compose/paths.c src/compose/state.c src/compose/table.c
src/xkbcomp/action.c src/xkbcomp/ast-build.c src/xkbcomp/compat.c src/xkbcomp/expr.c
src/xkbcomp/include.c src/xkbcomp/keycodes.c src/xkbcomp/keymap.c src/xkbcomp/keymap-dump.c
src/xkbcomp/keywords.c src/xkbcomp/rules.c src/xkbcomp/scanner.c src/xkbcomp/symbols.c
src/xkbcomp/types.c src/xkbcomp/vmod.c src/xkbcomp/xkbcomp.c
src/atom.c src/context.c src/context-priv.c src/keysym.c src/keysym-case-mappings.c
src/keysym-utf.c src/keymap.c src/keymap-compare.c src/keymap-priv.c src/rmlvo.c
src/scanner-utils.c src/state.c src/text.c src/utf8.c src/utf8-decoding.c src/utils.c
src/utils-paths.c"

# This folder: what the grammar was compiled to, and cache/ (downloads, not committed). Both
# scripts sourcing this are two folders down from the repository's top, as is this one.
xkb_dir=${XKB_DIR:-$(cd ../../third_party/xkbcommon && pwd)}

xkb_fetch() { # url sha file -> $xkb_dir/cache/file, checked
    if [ ! -f "$xkb_dir/cache/$3" ]; then
        mkdir -p "$xkb_dir/cache"
        curl -sSfL -o "$xkb_dir/cache/$3.part" "$1"
        mv "$xkb_dir/cache/$3.part" "$xkb_dir/cache/$3"
    fi
    echo "$2  $xkb_dir/cache/$3" | shasum -a 256 -c - >/dev/null || { echo "$3: checksum mismatch"; exit 1; }
}

xkb_prepare() { # work folder
    xkb_fetch "$XKB_URL" $XKB_SHA "libxkbcommon-$XKB_VERSION.tar.gz"
    xkb_fetch "$XKB_DATA_URL" $XKB_DATA_SHA "xkb-data_${XKB_DATA}_all.deb"
    mkdir -p "$1"
    # GitHub's archive is named for the tag (libxkbcommon-xkbcommon-1.13.2); called by the version.
    tar -C "$1" -xf "$xkb_dir/cache/libxkbcommon-$XKB_VERSION.tar.gz"
    mv "$1/libxkbcommon-xkbcommon-$XKB_VERSION" "$1/libxkbcommon-$XKB_VERSION"
    XKB_SRC=$1/libxkbcommon-$XKB_VERSION

    # The grammar, compiled by bison (3.6 or newer) beforehand: generate-parser.sh, which says
    # why. Made from this release's parser.y, or the build stops here.
    echo "$(cat "$xkb_dir/parser.y.sha256")  $XKB_SRC/src/xkbcomp/parser.y" | shasum -a 256 -c - >/dev/null ||
        { echo "third_party/xkbcommon/parser.c is not $XKB_VERSION's grammar: run generate-parser.sh"; exit 1; }
    cp "$xkb_dir/parser.c" "$xkb_dir/parser.h" "$1/"

    # What meson.build's configh_data holds for a build with no X11, no tools and no xkbregistry,
    # where the layouts live by default, and where Compose's tables do. None of the defaults is
    # used: the Mac app and the Android one point XKB_CONFIG_ROOT at their own copy of the
    # layouts, and the clients read the compositor's keymap and look in no folder. No Compose
    # tables are carried (SpatiMesh asks for its locale's, finds none, and goes without, as on 1.7).
    cat > "$1/config.h" <<EOF
#define EXIT_INVALID_USAGE 2
#define LIBXKBCOMMON_VERSION "$XKB_VERSION"
#define LIBXKBCOMMON_TOOL_PATH ""
#define DFLT_XKB_LEGACY_ROOT "/nonexistent/xkb"
#define DFLT_XKB_CONFIG_ROOT "/nonexistent/xkb"
#define DFLT_XKB_CONFIG_EXTRA_PATH "/nonexistent/xkb-extra"
#define XLOCALEDIR "/nonexistent/locale"
#define DEFAULT_XKB_RULES "evdev"
#define DEFAULT_XKB_MODEL "pc105"
#define DEFAULT_XKB_LAYOUT "us"
#define DEFAULT_XKB_VARIANT NULL
#define DEFAULT_XKB_OPTIONS NULL
#define HAVE_UNISTD_H 1
#define HAVE_DIRENT_H 1
#define HAVE_XKB_EXTENSIONS_DIRECTORIES 1
#define HAVE___BUILTIN_EXPECT 1
#define HAVE_MMAP 1
#define HAVE_STRNDUP 1
#define HAVE_ASPRINTF 1
EOF
    XKB_CFLAGS="-O2 -fPIC -std=c11 -fno-strict-aliasing -fvisibility=default -include $1/config.h
        -I $1 -I $XKB_SRC -I $XKB_SRC/include -I $XKB_SRC/src -I $XKB_SRC/src/xkbcomp -w"

    # The layouts: rules, keycodes, types, compat and symbols, as the package installs them.
    mkdir -p "$1/deb"
    (cd "$1/deb" && ar x "$xkb_dir/cache/xkb-data_${XKB_DATA}_all.deb" && tar -xf data.tar.xz)
    # xkeyboard-config 2.45 and later put the layouts in /usr/share/xkeyboard-config-2 and leave
    # /usr/share/X11/xkb a link to it; taken whole, as a folder, either way.
    if [ -d "$1/deb/usr/share/xkeyboard-config-2" ]; then
        cp -R "$1/deb/usr/share/xkeyboard-config-2" "$1/xkb"
    else
        cp -R "$1/deb/usr/share/X11/xkb" "$1/xkb"
    fi
}
