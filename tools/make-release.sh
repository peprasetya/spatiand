#!/usr/bin/env bash
# Build the Steam Deck release: a folder somebody downloads, unpacks and installs by double-clicking.
#
# The shape is deliberate. What arrives is a folder with an installer you double click, the
# programs it installs, and text files -- because the audience for this is somebody with a Steam
# Deck and a pair of glasses, not somebody with a Rust toolchain. Anything that requires reading
# build instructions before seeing the thing work has lost most of the people it was for.
#
# Run it on the Deck, inside the build container, from a clean copy of the commit being released --
# not from the Deck's development checkout, which holds whatever was last synced to it:
#
#   git archive --prefix=src/ HEAD | ssh deck@steamdeck 'rm -rf ~/spatiand-release/src && mkdir -p ~/spatiand-release && tar -x -C ~/spatiand-release'
#   ssh deck@steamdeck "distrobox enter holo -- env SPATIAND_COMMIT=$(git rev-parse HEAD) nice -n 19 ~/spatiand-release/src/tools/make-release.sh"
#
# It builds into target/steamdeck with flags of its own (the programs find their libraries in
# ../lib, wherever they are put), so it never reads or disturbs a development build or its
# .cargo/config.toml. Cargo.lock's crates, exactly (--locked), come from cargo's cache as the
# development build left it; not --offline, because the Deck's index cache is partly an old
# container's and stale, and resolving needs crates.io's index as the development build does. It writes
# dist/spatiand-<version>-steamdeck.tar.gz and its .sha256. What goes in besides this tree, each
# from where the Deck's own build has it unless named:
#
#   SPATIAND_VERSION    the version: today's date
#   SPATIAND_COMMIT     the commit this is (git's, where this is a checkout)
#   SPATIAND_FFMPEG     Spatiand's FFmpeg, `third_party/ffmpeg/build.sh linux`'s out/linux
#   SPATIAND_FFMPEG_SOURCE  that FFmpeg's source, for its licence texts (its src/ffmpeg-<version>)
#   SPATIAND_CHROMIUM   SpatiMesh, `third_party/chromium/build.sh`'s package (~/spatiand-chromium)
#   SPATIAND_STORAGE    the storage service, `storage/build.sh linux`: built here when Go is on the
#                       PATH; else the program, with what `tools/release-licences.py go` wrote of it
#                       beside it (<program>.modules and <program>.licences) where it was built
#   SPATIAND_LIBVA_HEADERS, SPATIAND_LINK_DIR  what the container lacks to build against SteamOS's
#                       libva: its headers, and the libraries' link names (~/spatiand-build)
#   SPATIAND_STRIP=0    SpatiMesh as packaged, with its symbols (some 550 MB more)
set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO"
VERSION="${SPATIAND_VERSION:-$(date +%Y-%m-%d)}"
NAME="spatiand-$VERSION-steamdeck"
DIST="$REPO/dist"
OUT="$DIST/$NAME"
P="$OUT/spatiand"
TARGET="$REPO/target/steamdeck"

die() { echo "!! $*" >&2; exit 1; }
# The first of these folders that exists, else the first.
pick() { local d; for d in "$@"; do [ -d "$d" ] && { echo "$d"; return; }; done; echo "$1"; }

COMMIT="${SPATIAND_COMMIT:-$(git -C "$REPO" rev-parse HEAD 2>/dev/null || true)}"
[ -n "$COMMIT" ] || die "which commit is this? Set SPATIAND_COMMIT"
. third_party/ffmpeg/versions.sh
CHROMIUM_VERSION=$(sed -n 's/^CHROMIUM_VERSION=\([^ ]*\).*/\1/p' third_party/chromium/build.sh)
FFMPEG="${SPATIAND_FFMPEG:-$(pick "$REPO/third_party/ffmpeg/out/linux" "$HOME/spatiand/third_party/ffmpeg/out/linux")}"
FFMPEG_SOURCE="${SPATIAND_FFMPEG_SOURCE:-$(pick "$REPO/third_party/ffmpeg/src/ffmpeg-$FFMPEG_VERSION" "$HOME/spatiand/third_party/ffmpeg/src/ffmpeg-$FFMPEG_VERSION")}"
CHROMIUM="${SPATIAND_CHROMIUM:-$HOME/spatiand-chromium}"
LIBVA_HEADERS="${SPATIAND_LIBVA_HEADERS:-$HOME/spatiand-build/libva-headers}"
LINK_DIR="${SPATIAND_LINK_DIR:-$HOME/spatiand-build/valib}"
CARGO="${CARGO:-$(command -v cargo || echo "$HOME/.cargo/bin/cargo")}"

# The programs, as own.rs and the session start them (spatiand-choose is the file chooser SpatiMesh
# asks where there is no desktop portal; spatikey-host is how other browsers reach SpatiKey).
PACKAGES="spatiand spatishelf spatiquill spatireel spatiloupe spatipal spatikey spatiand-choose"
PROGRAMS="spatiand spatishelf spatiquill spatireel spatiloupe spatipal spatikey spatikey-host spatiand-choose"

echo "== checking what goes in =="
[ -x "$CARGO" ] || die "no cargo found; set CARGO=/path/to/cargo"
[ -f "$FFMPEG/lib/libavutil.so" ] && [ -d "$FFMPEG/include" ] || die "no FFmpeg at $FFMPEG: third_party/ffmpeg/build.sh linux, or set SPATIAND_FFMPEG"
FFMPEG_CONFIGURATION=$(strings "$FFMPEG/lib/libavutil.so" | grep -m1 -- '^--prefix=' || true)
FFMPEG_LICENCE=$(strings "$FFMPEG/lib/libavutil.so" | sed -n 's/^libavutil license: //p' | head -1 || true)
# What NOTICES.md says it is, and what the build refuses to be otherwise.
case "$FFMPEG_LICENCE" in
    "LGPL version 3 or later") ;;
    *) die "FFmpeg at $FFMPEG says its licence is '$FFMPEG_LICENCE', not LGPL version 3 or later" ;;
esac
[ -f "$FFMPEG_SOURCE/COPYING.LGPLv3" ] || die "no FFmpeg source at $FFMPEG_SOURCE, for its licence; set SPATIAND_FFMPEG_SOURCE"
# The BoringSSL inside FFmpeg's libraries: its licence, and the commit it was built from, come with them.
[ -f "$FFMPEG/share/boringssl/LICENSE" ] || die "no BoringSSL licence in $FFMPEG/share/boringssl: an FFmpeg built before its https was BoringSSL's? third_party/ffmpeg/build.sh linux"
BORINGSSL_REVISION=$(sed -n 's/.* boringssl \([^ ]*\).*/\1/p' "$FFMPEG/.version")
[ -x "$CHROMIUM/spatimesh" ] && [ -f "$CHROMIUM/run.sh" ] || die "no SpatiMesh at $CHROMIUM: third_party/chromium/build.sh's package, or set SPATIAND_CHROMIUM"
# Read, not run: the container has not the libraries a browser needs.
grep -qaF "$CHROMIUM_VERSION" "$CHROMIUM/spatimesh" ||
    die "SpatiMesh at $CHROMIUM is not Chromium $CHROMIUM_VERSION, this tree's (third_party/chromium/build.sh)"
chromium_rpath=$(readelf -d "$CHROMIUM/spatimesh" | sed -n 's/.*(R.*PATH).*\[\(.*\)\]/\1/p')
[[ ":$chromium_rpath:" == *':$ORIGIN/ffmpeg:'* ]] ||
    die "SpatiMesh at $CHROMIUM does not look for FFmpeg in \$ORIGIN/ffmpeg (patch 0003): it would not find this release's"
[ -d "$LIBVA_HEADERS" ] && [ -d "$LINK_DIR" ] || die "no libva headers ($LIBVA_HEADERS) or link names ($LINK_DIR): set SPATIAND_LIBVA_HEADERS and SPATIAND_LINK_DIR"

rm -rf "$OUT"
install -d "$P/bin" "$P/lib" "$P/licences" "$P/tools" "$P/assets/apps" "$P/assets/icons"

STORAGE="${SPATIAND_STORAGE:-}"
if [ -z "$STORAGE" ]; then
    command -v go >/dev/null || die "no Go here to build the storage service with: build it where Go is (storage/build.sh linux <program>) and set SPATIAND_STORAGE"
    echo "== building the storage service =="
    STORAGE="$TARGET/spatiand-storage"
    mkdir -p "$TARGET"
    storage/build.sh linux "$STORAGE"
    python3 -I tools/release-licences.py go "$STORAGE" --modcache "$(go env GOMODCACHE)" --list "$STORAGE.modules" --texts "$STORAGE.licences"
fi
[ -x "$STORAGE" ] || die "no storage service at $STORAGE"
grep -qa "rclone/rclone" "$STORAGE" || die "$STORAGE is not the storage service (no rclone in it)"
STORAGE_RCLONE=$(python3 -I - "$STORAGE" <<'PY'
import re, sys
m = re.search(rb"\ndep\tgithub.com/rclone/rclone\t([^\t\n]+)", open(sys.argv[1], "rb").read())
print(m.group(1).decode() if m else "")
PY
)
STORAGE_GO=$(strings "$STORAGE" | grep -m1 -oE '^go1\.[0-9]+(\.[0-9]+)?$' || true)

echo "== building $PROGRAMS =="
# The Deck's own flags (its .cargo/config.toml), less where FFmpeg is: $ORIGIN/../lib, an RPATH and
# not a RUNPATH, as FFmpeg's libraries load each other and only an RPATH reaches them.
packages=()
for p in $PACKAGES; do packages+=(-p "$p"); done
env CARGO_TARGET_DIR="$TARGET" \
    FFMPEG_INCLUDE_DIR="$FFMPEG/include" FFMPEG_LIBS_DIR="$FFMPEG/lib" FFMPEG_LINK_MODE=dynamic \
    CROS_LIBVA_H_PATH="$LIBVA_HEADERS" \
    RUSTFLAGS="-L $LINK_DIR -C link-arg=-Wl,--disable-new-dtags,-rpath,\$ORIGIN/../lib" \
    "$CARGO" build --release --locked "${packages[@]}"
RUST_VERSION=$("$CARGO" --version | awk '{print $2}')

echo "== packaging $NAME =="
for p in $PROGRAMS; do
    install -m 755 "$TARGET/release/$p" "$P/bin/$p"
    rpath=$(readelf -d "$P/bin/$p" | sed -n 's/.*(RPATH).*\[\(.*\)\]/\1/p')
    [ "$rpath" = '$ORIGIN/../lib' ] || die "$p looks for its libraries in '$rpath', not \$ORIGIN/../lib"
done
install -m 755 "$STORAGE" "$P/bin/spatiand-storage"

# SpatiMesh as third_party/chromium/build.sh packages it, with this tree's launcher, less what it
# made of its own at run time; its symbols kept only when asked for.
cp -a "$CHROMIUM" "$P/lib/spatimesh"
rm -rf "$P/lib/spatimesh/extensions"
install -m 755 third_party/chromium/run.sh "$P/lib/spatimesh/run.sh"
if [ "${SPATIAND_STRIP:-1}" != 0 ]; then
    strip --strip-unneeded "$P/lib/spatimesh/spatimesh" "$P/lib/spatimesh/chrome_crashpad_handler"
fi

# FFmpeg's libraries that the programs and the browser load, and the ones those load in turn,
# under the names they are loaded by.
needed() { readelf -d "$@" 2>/dev/null | sed -n 's/.*(NEEDED).*\[\(lib\(av\|sw\)[^]]*\)\]/\1/p' | sort -u; }
want=$(needed "$P"/bin/* "$P/lib/spatimesh/spatimesh")
while :; do
    for soname in $want; do
        [ -f "$P/lib/$soname" ] || cp -L "$FFMPEG/lib/$soname" "$P/lib/$soname" || die "no $soname in $FFMPEG/lib"
    done
    more=$(needed "$P"/lib/lib*.so.*)
    [ -z "$(comm -13 <(echo "$want") <(echo "$more"))" ] && break
    want=$(sort -u <(echo "$want") <(echo "$more"))
done
chmod 755 "$P"/lib/lib*.so.*
# The browser's own place for them (patch 0003's $ORIGIN/ffmpeg), pointing at the one copy.
mkdir "$P/lib/spatimesh/ffmpeg"
for l in "$P"/lib/lib*.so.*; do ln -s "../../$(basename "$l")" "$P/lib/spatimesh/ffmpeg/"; done

# What the installer runs, and what is run after it: the same scripts as a development checkout's,
# which know the release's layout (bin, lib, tools and assets side by side).
install -m 644 assets/apps/*.svg "$P/assets/apps/"
install -m 644 assets/icons/spatiand.svg "$P/assets/icons/"
for t in install-desktop-apps.sh install-desktop-link.sh install-session.sh install-udev-rules.sh go-to-spatiand.sh; do
    install -m 755 "tools/$t" "$P/tools/$t"
done
install -m 755 tools/uninstall-release.sh "$P/uninstall.sh"
GLIBC=$(ldd --version | head -1 | grep -oE '[0-9]+\.[0-9]+$' || true)
STEAMOS=$(sed -n 's/^VERSION_ID=//p' /run/host/etc/os-release 2>/dev/null || true)
cat > "$P/VERSION" <<EOF
version=$VERSION
commit=$COMMIT
glibc=$GLIBC
steamos=${STEAMOS:-3.8}
EOF

echo "== licences =="
cp LICENSE "$P/licences/LICENSE"
cp third_party/NOTICES.md "$P/licences/NOTICES.md"
cp "$FFMPEG_SOURCE/COPYING.LGPLv3" "$P/licences/FFmpeg-COPYING.LGPLv3"
cp "$FFMPEG/share/boringssl/LICENSE" "$P/licences/BoringSSL-LICENSE.txt"
cp third_party/chromium/patches/base/COPYING "$P/licences/COPYING.GPLv3"
"$CARGO" metadata --format-version 1 --locked --filter-platform x86_64-unknown-linux-gnu >"$TARGET/metadata.json"
python3 -I tools/release-licences.py rust "$TARGET/metadata.json" $PACKAGES \
    --list "$TARGET/rust-crates.list" --texts "$P/licences/rust-crates.txt"
if [ -f "$STORAGE.modules" ]; then
    cp "$STORAGE.modules" "$TARGET/go-modules.list"
    [ -f "$STORAGE.licences" ] && cp "$STORAGE.licences" "$P/licences/go-modules.txt"
else
    python3 -I tools/release-licences.py go "$STORAGE" --list "$TARGET/go-modules.list" --texts "$TARGET/go-modules.txt"
fi

cat > "$OUT/THIRD-PARTY.txt" <<EOF
Spatiand $VERSION for the Steam Deck: what is inside, and under which licence
===============================================================================

Spatiand's own code is MIT (LICENSE). It carries, as separate libraries and programs beside its
own, the software below, each under its own licence. The licence texts are in spatiand/licences/
(installed, ~/.local/share/spatiand/release/licences/), with the project's own notice for every
platform, NOTICES.md.

FFmpeg $FFMPEG_VERSION -- spatiand/lib/libav*.so.*, libsw*.so.*
    LGPL version 3 or later (licences/FFmpeg-COPYING.LGPLv3, which incorporates
    licences/COPYING.GPLv3). Linked dynamically: these are ordinary shared libraries, and can be
    replaced with a build of one's own of the same version. Built from
    https://ffmpeg.org/releases/ffmpeg-$FFMPEG_VERSION.tar.xz, with the patch in
    third_party/ffmpeg/patches/ffmpeg (its TLS code on BoringSSL), by third_party/ffmpeg/build.sh,
    configured as:
        $FFMPEG_CONFIGURATION
    without --enable-gpl or --enable-nonfree; --enable-version3 because the BoringSSL built into
    it is under the Apache licence, which is compatible with LGPL 3, not 2.1. Its zlib, bzip2,
    Opus, libva, libdrm and C++ runtime are SteamOS's own, not carried here.

BoringSSL -- inside FFmpeg's libraries (libavformat.so.*, libavutil.so.*), for its https
    Apache 2.0 (licences/BoringSSL-LICENSE.txt). SpatiMesh's own, built from commit
    $BORINGSSL_REVISION of its Chromium's third_party/boringssl/src: the revision that
    Chromium's DEPS names (https://boringssl.googlesource.com/boringssl), with SpatiMesh's patch
    to it (third_party/chromium/patches/sub/boringssl) committed on it.

SpatiMesh, the web browser -- spatiand/lib/spatimesh/ -- which also shows SpatiLoupe's PDFs
    Chromium $CHROMIUM_VERSION, with the patches in third_party/chromium of Spatiand's
    repository:
      * Chromium: BSD 3-Clause and the licences of its parts, which the browser lists at
        chrome://credits (among them its PDF viewer's PDFium, ANGLE's libEGL.so and
        libGLESv2.so, SwiftShader's libvk_swiftshader.so and the Vulkan loader libvulkan.so.1,
        carried beside it).
      * Cromite's patches, taken over at Chromium 153 (patches/base, patches/sub), with eyeo's
        Chromium SDK (the ad blocker) among them: GPL-3.0 (licences/COPYING.GPLv3).
      * Spatiand's own patches: MIT, as the rest of Spatiand; where one changes a GPL file of the
        above, that file stays GPL-3.0.
    SpatiMesh as built is therefore under the GPL-3.0. Its complete corresponding source is
    Chromium $CHROMIUM_VERSION plus those patches, applied by third_party/chromium/build.sh:
    SOURCE.txt says where each is. It finds this release's FFmpeg through lib/spatimesh/ffmpeg.

The storage service -- spatiand/bin/spatiand-storage
    rclone $STORAGE_RCLONE (MIT), linked in as a Go module, unchanged, with the Go modules below
    (storage/go.mod and storage/go.sum pin them), built with $STORAGE_GO. One of them,
    github.com/cloudsoda/sddl (Windows security descriptors, which rclone's SMB backend reads), is
    LGPL-3.0: the service is built from public source (storage/ in the repository), so it can be
    built again with a changed copy of that module. Their licence texts: licences/go-modules.txt.

Rust crates compiled into Spatiand's programs -- spatiand/bin/*
    The crates below, each under the licence it declares (where it offers a choice, "OR", under
    any one of them), from crates.io as Cargo.lock pins them, with rsmpeg patched
    (third_party/rsmpeg, MIT). Their licence texts: licences/rust-crates.txt.

Not carried, but used where SteamOS has them: libmysofa and its default.sofa (the room's spatial
sound), PipeWire, Mesa, libinput, libseat, xkbcommon and the rest that ldd lists.

-------------------------------------------------------------------------------
Go modules in the storage service: module, version, licence
-------------------------------------------------------------------------------
$(cat "$TARGET/go-modules.list")

-------------------------------------------------------------------------------
Rust crates in Spatiand's programs: crate, version, licence
-------------------------------------------------------------------------------
$(cat "$TARGET/rust-crates.list")
EOF
cp "$OUT/THIRD-PARTY.txt" "$P/licences/THIRD-PARTY.txt"

stripped=""
[ "${SPATIAND_STRIP:-1}" = 0 ] || stripped=",
    with its debugging symbols stripped (strip --strip-unneeded) for this release"
cat > "$OUT/SOURCE.txt" <<EOF
Spatiand $VERSION for the Steam Deck: where every part's source is, and how it was built
===============================================================================

Spatiand: https://github.com/peprasetya/spatiand, commit $COMMIT
    https://github.com/peprasetya/spatiand/tree/$COMMIT
Every path below is in that tree.

Spatiand's programs (spatiand/bin: spatiand, spatishelf, spatiquill, spatireel, spatiloupe,
spatipal, spatikey, spatikey-host, spatiand-choose)
    Rust $RUST_VERSION (rust-toolchain.toml), with the crates Cargo.lock pins, built by
    tools/make-release.sh on a Steam Deck (SteamOS ${STEAMOS:-3.8}), in an Arch Linux container with
    glibc $GLIBC: cargo build --release --locked -p <each of $PACKAGES>,
    against the FFmpeg below and SteamOS's libva, with \$ORIGIN/../lib as where they find it.

The storage service (spatiand/bin/spatiand-storage)
    storage/ (main.go, go.mod, go.sum), built by storage/build.sh linux with $STORAGE_GO:
    rclone $STORAGE_RCLONE and its modules, CGO off, one program with no C in it.

FFmpeg $FFMPEG_VERSION (spatiand/lib/libav*, libsw*)
    https://ffmpeg.org/releases/ffmpeg-$FFMPEG_VERSION.tar.xz, unchanged (third_party/ffmpeg/patches
    has none), built by third_party/ffmpeg/build.sh linux (versions in third_party/ffmpeg/versions.sh),
    configured as:
        $FFMPEG_CONFIGURATION

SpatiMesh (spatiand/lib/spatimesh) -- GPL-3.0, so this is its corresponding source
    Chromium $CHROMIUM_VERSION: https://chromium.googlesource.com/chromium/src/+/refs/tags/$CHROMIUM_VERSION
    (with the dependencies its DEPS names), and third_party/chromium: patches/base and
    patches/sub (Cromite's), patches/0*.patch (Spatiand's), args.gn, and build.sh, which fetches
    that version, applies the patches in order and builds and packages it:
        third_party/chromium/build.sh <work folder> all
    The browser here is that package (lib/spatimesh/run.sh is third_party/chromium/run.sh)$stripped.

The installer and what it installs (install.sh, uninstall.sh, spatiand/tools)
    tools/install-release.sh, tools/uninstall-release.sh, and the tools/ scripts of the same
    names; the menu's pictures are assets/.
EOF
cp "$OUT/SOURCE.txt" "$P/SOURCE.txt"

echo "== the installer =="
install -m 755 tools/install-release.sh "$OUT/install.sh"
install -m 755 tools/uninstall-release.sh "$OUT/uninstall.sh"
cp LICENSE "$OUT/LICENSE"
cp docs/install.md "$OUT/README.txt"
# The clickable halves. `Terminal=true` on purpose: the install says what it is doing and what it
# could not do, and a graphical launcher that fails silently is worse than no launcher. %k is this
# file, so the folder it is in is the release's wherever it was unpacked.
clickable() { # file, name, comment, script, icon
    cat > "$OUT/$1" <<EOF
[Desktop Entry]
Type=Application
Name=$2
Comment=$3
Exec=bash -c 'cd "\$(dirname "%k")" && ./$4; echo; read -rp "Press Enter to close this window."'
Icon=$5
Terminal=true
Categories=System;
EOF
    chmod +x "$OUT/$1"
}
clickable "Install Spatiand.desktop" "Install Spatiand" "Install the spatial desktop, its applications, and Spatial Mode at the login screen" install.sh system-software-install
clickable "Uninstall Spatiand.desktop" "Uninstall Spatiand" "Remove Spatiand, keeping what you made with it" uninstall.sh edit-delete

echo "== tarball =="
cd "$DIST"
rm -f "$NAME.tar.gz" "$NAME.tar.gz.sha256"
compress=$(command -v pigz || command -v gzip)
tar --owner=0 --group=0 --numeric-owner --sort=name -cf - "$NAME" | "$compress" -9 > "$NAME.tar.gz"
sha256sum "$NAME.tar.gz" > "$NAME.tar.gz.sha256"
echo
echo "wrote dist/$NAME.tar.gz ($(du -h "$NAME.tar.gz" | cut -f1), unpacked $(du -sh "$NAME" | cut -f1))"
cat "$NAME.tar.gz.sha256"
