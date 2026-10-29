#!/bin/bash
# SpatiMesh's Chromium: Chromium at the version Cromite's patches are made for, Cromite's patches,
# then Spatiand's own (patches/, in order), built for Linux (the Deck) as Cromite builds it.
#
#   third_party/chromium/build.sh <work folder> [fetch|patch|build|package|all]
#
# Run on a Linux machine with ~120 GB free (deepmagpie: /media/shared/Linux/App/spatiand-chromium).
# Each step runs inside an Ubuntu 22.04 root (as Cromite's own build image is), given as
# CHROMIUM_ROOT (default: SpatiWorld's), with the work folder at /cr; bubblewrap, no root needed.
# The result is <work>/package/spatiand-chromium.tar.gz: the browser and a launcher (run.sh)
# that opens it as a Wayland window of the room.
set -e

# Every pinned version, in one place.
CROMITE_COMMIT=3724efc6ede3d2d94611c1fe71d1b090ebbb40a7   # 2026-09-19, Chromium 153.0.8010.37
DEPOT_TOOLS_COMMIT=7083aadfb

work=$(cd "${1:?work folder}" && pwd); step=${2:-all}
here=$(cd "$(dirname "$0")" && pwd)
root=${CHROMIUM_ROOT:-/media/shared/Linux/App/SpatiWorld/buildenv-jammy/root}

inside() { # command...: run in the 22.04 root, the work folder at /cr
    bwrap --unshare-user --uid 1000 --gid 1000 --bind "$root" / --dev /dev --tmpfs /dev/shm --proc /proc \
        --bind /tmp /tmp --bind "$work" /cr --ro-bind /etc/resolv.conf /etc/resolv.conf \
        --setenv HOME /cr/home --setenv USER builder --setenv LOGNAME builder \
        --setenv PATH /cr/depot_tools:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
        --setenv DEPOT_TOOLS_UPDATE 0 --setenv DEPOT_TOOLS_METRICS 0 --chdir /cr "$@"
}

fetch() {
    mkdir -p "$work/home" "$work/logs"
    [ -d "$work/depot_tools" ] || git clone -q https://chromium.googlesource.com/chromium/tools/depot_tools.git "$work/depot_tools"
    git -C "$work/depot_tools" checkout -q "$DEPOT_TOOLS_COMMIT"
    [ -d "$work/cromite" ] || git clone -q https://github.com/uazo/cromite.git "$work/cromite"
    git -C "$work/cromite" fetch -q origin "$CROMITE_COMMIT" 2>/dev/null || true
    git -C "$work/cromite" checkout -q "$CROMITE_COMMIT"
    inside bash -s <<'EOF'
set -e
(cd /cr/depot_tools && ./ensure_bootstrap >/dev/null)   # gn and siso want it; DEPOT_TOOLS_UPDATE=0 skips it
VERSION=$(cat /cr/cromite/build/RELEASE)
URL=https://chromium.googlesource.com/chromium/src.git
mkdir -p /cr/chromium/src && cd /cr/chromium/src
[ -d .git ] || git init -q
git remote get-url origin >/dev/null 2>&1 || git remote add origin $URL
git fetch --depth 2 origin "+refs/tags/$VERSION:refs/tags/$VERSION"
git checkout -q "tags/$VERSION"
cat > ../.gclient <<G
solutions = [
  { "name": "src", "url": "$URL@$(git rev-parse HEAD)", "deps_file": "DEPS", "managed": False,
    "custom_deps": {
      "src/third_party/gperf": None, "src/third_party/lighttpd": None, "src/third_party/perl": None,
      "src/testing/libfuzzer/fuzzers/wasm_corpus": None,
      "src/third_party/cronet_android_mainline_clang/linux-amd64": None,
    },
    "custom_hooks": [
      { "name": "ciopfs_linux", "pattern": ".", "action": ["echo", "skipped"] },
      { "name": "win_toolchain", "pattern": ".", "action": ["echo", "skipped"] },
      { "name": "rc_win", "pattern": ".", "action": ["echo", "skipped"] },
      { "name": "rc_linux", "pattern": ".", "action": ["echo", "skipped"] },
    ],
    "custom_vars": { "checkout_telemetry_dependencies": False, "checkout_pgo_profiles": True },
  },
]
G
git config user.email "spatiand@localhost"; git config user.name "Spatiand build"
# Three at a time: googlesource answers HTTP 429 to eight; tried again while it refuses.
for attempt in 1 2 3 4 5 6 7 8; do
    gclient sync -D --no-history --nohooks -j 3 && gclient runhooks && break
    [ $attempt = 8 ] && exit 1
    sleep 120
done
rm -rf third_party/angle/third_party/VK-GL-CTS/
EOF
}

patch_tree() {
    inside bash -s <<'EOF'
set -e
cd /cr
# Cromite's own script (it folds v8, devtools-frontend, skia, perfetto and boringssl into the
# main repository, which its patches need), without its apt step. gclient cannot sync this tree
# afterwards: fetch again into a fresh folder for another version.
sed '/^sudo apt install sed/d' /cr/cromite/tools/images/cromite-source/apply-cromite-patches.sh | bash
cd /cr/chromium/src
for p in /cr/spatiand-patches/*.patch; do git am -q "$p"; done
EOF
}

build() {
    # Spatiand's own FFmpeg (third_party/ffmpeg, `build.sh linux` -- 9.0.2, one copy for every
    # program of Spatiand's): its headers and libraries where build/linux/unbundle/ffmpeg.gn
    # (patch 0003) looks for them.
    ffmpeg=${SPATIAND_FFMPEG:-$here/../ffmpeg/out/linux}
    staged=$work/chromium/src/third_party/spatiand_ffmpeg
    rm -rf "$staged"; mkdir -p "$staged/lib"
    cp -a "$ffmpeg/include" "$staged/"
    cp -a "$ffmpeg"/lib/libav*.so* "$staged/lib/"
    inside bash -s <<'EOF'
set -e
cd /cr/chromium/src
# Chromium's own way to build on an FFmpeg of the system's: //third_party/ffmpeg becomes the shim.
grep -q spatiand_ffmpeg third_party/ffmpeg/BUILD.gn || python3 build/linux/unbundle/replace_gn_files.py --system-libraries ffmpeg
# Cromite's Linux arguments; Spatiand's changes made in place (GN refuses a value set twice):
# no AV1 offered while there is no hardware decoding of it through Spatiand's FFmpeg (the owner
# wants no software AV1, and an AV1 claimed without a decoder stalls YouTube).
ARGS=$(sed -e "s/^enable_dav1d_decoder=true/enable_dav1d_decoder=false/" -e "s/^enable_av1_decoder=true/enable_av1_decoder=false/" /cr/cromite/build/cromite.gn_args)
# Dolby Digital and Dolby Digital Plus, decoded by Spatiand's FFmpeg.
ARGS="$ARGS
enable_platform_ac3_eac3_audio = true"
gn gen --args="target_os = \"linux\" $ARGS target_cpu = \"x64\" " out/deck
# The machine also serves spatiand-host: the lowest priority.
nice -n 19 ionice -c 3 python3 /cr/depot_tools/siso.py ninja -C out/deck chrome chrome_sandbox --offline
cp /cr/cromite/build/RELEASE out/deck/
EOF
}

package() {
    O=$work/chromium/src/out/deck; P=$work/package/spatiand-chromium
    rm -rf "$P"; mkdir -p "$P"
    # What chrome/installer/linux/BUILD.gn packages, with every locale.
    for f in chrome chrome_crashpad_handler libEGL.so libGLESv2.so libvulkan.so.1 libvk_swiftshader.so \
        vk_swiftshader_icd.json libqt5_shim.so libqt6_shim.so icudtl.dat snapshot_blob.bin v8_context_snapshot.bin \
        RELEASE xdg-mime xdg-settings; do
        [ -e "$O/$f" ] && cp -a "$O/$f" "$P/"
    done
    cp -a "$O"/*.pak "$P/"
    for d in locales MEIPreload PrivacySandboxAttestationsPreloaded; do [ -d "$O/$d" ] && cp -a "$O/$d" "$P/"; done
    cp "$work/chromium/src/chrome/app/theme/chromium/product_logo_256.png" "$P/icon.png"
    cp "$here/run.sh" "$P/run.sh"
    tar -C "$work/package" -czf "$work/package/spatiand-chromium.tar.gz" spatiand-chromium
    echo "packaged $(du -sh "$P" | cut -f1) in $work/package/spatiand-chromium.tar.gz"
}

mkdir -p "$work/spatiand-patches"
cp "$here"/patches/*.patch "$work/spatiand-patches/"
case $step in
    fetch) fetch ;;
    patch) patch_tree ;;
    build) build ;;
    package) package ;;
    all) fetch; patch_tree; build; package ;;
    *) echo "unknown step: $step" >&2; exit 2 ;;
esac
