#!/bin/bash
# SpatiMesh's Chromium: Chromium at the version Cromite's patches are made for, Cromite's patches,
# then Spatiand's own (patches/, in order), built as Cromite builds it: for Linux (the Deck), or
# with CHROMIUM_TARGET=android for Android on arm64 (the Beam Pro).
#
#   [CHROMIUM_TARGET=android] third_party/chromium/build.sh <work folder> [fetch|patch|build|package|all]
#
# Run on a Linux machine with ~120 GB free (deepmagpie: /media/shared/Linux/App/spatiand-chromium,
# and spatiand-chromium-android: a tree cannot be synced for another target once the patches are in).
# Each step runs inside an Ubuntu 22.04 root (as Cromite's own build image is), given as
# CHROMIUM_ROOT (default: SpatiWorld's), with the work folder at /cr; bubblewrap, no root needed.
# The result is <work>/package/spatiand-chromium.tar.gz: the browser and a launcher (run.sh)
# that opens it as a Wayland window of the room. For Android, the same browser built for Android's
# libc (android-libc/), <work>/package/spatimesh-android.tar.gz, for Spatiand's APK to carry.
set -e

# Every pinned version, in one place.
CROMITE_COMMIT=3724efc6ede3d2d94611c1fe71d1b090ebbb40a7   # 2026-09-19, Chromium 153.0.8010.37
DEPOT_TOOLS_COMMIT=7083aadfb

work=$(cd "${1:?work folder}" && pwd); step=${2:-all}
here=$(cd "$(dirname "$0")" && pwd)
root=${CHROMIUM_ROOT:-/media/shared/Linux/App/SpatiWorld/buildenv-jammy/root}
target=${CHROMIUM_TARGET:-linux}
case $target in linux|android) ;; *) echo "unknown target: $target" >&2; exit 2 ;; esac

inside() { # command...: run in the 22.04 root, the work folder at /cr
    bwrap --unshare-user --uid 1000 --gid 1000 --bind "$root" / --dev /dev --tmpfs /dev/shm --proc /proc \
        --bind /tmp /tmp --bind "$work" /cr --ro-bind /etc/resolv.conf /etc/resolv.conf \
        --setenv HOME /cr/home --setenv USER builder --setenv LOGNAME builder --setenv CHROMIUM_TARGET "$target" \
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
# Android's SDK, NDK and the rest of its toolchain come with the target; a Linux build needs none.
ANDROID_VARS=; TARGET_OS=
if [ "$CHROMIUM_TARGET" = android ]; then
    ANDROID_VARS='"checkout_android_prebuilts_build_tools": True, '
    TARGET_OS='target_os = ["android"]'
fi
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
    "custom_vars": { ${ANDROID_VARS}"checkout_telemetry_dependencies": False, "checkout_pgo_profiles": True },
  },
]
$TARGET_OS
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
# For Android's libc, what Termux changes and then ours (android-libc/README.md). With patch(1),
# fuzz and all, as Termux applies them: some reach into dependencies that are repositories of
# their own, where git am cannot follow.
if [ "$CHROMIUM_TARGET" = android ]; then
    for p in /cr/android-libc/termux/*.patch /cr/android-libc/0*.patch; do
        patch -p1 -F 3 --no-backup-if-mismatch -s < "$p"
    done
fi
EOF
}

build_android() {
    # Spatiand's FFmpeg for Android (third_party/ffmpeg, `build.sh android`: 9.0.2 with MediaCodec in
    # it), where patch 0003 looks for it, as for Linux; the APK carries the same libraries.
    ffmpeg=${SPATIAND_FFMPEG:-$here/../ffmpeg/out/android}
    staged=$work/chromium/src/third_party/spatiand_ffmpeg
    rm -rf "$staged"; mkdir -p "$staged/lib"
    cp -a "$ffmpeg/include" "$staged/"
    cp -a "$ffmpeg"/lib/libav*.so* "$ffmpeg"/lib/libsw*.so* "$staged/lib/"
    # Android's libc at API 24, which Termux's changes are written for, and the one library Chromium
    # takes from the system here: xkbcommon, as the WPE build made it for Android (the APK carries it).
    # Everything else it builds itself (fontconfig, libffi, libwayland, libdrm).
    deps=${SPATIAND_ANDROID_DEPS:-/media/shared/Linux/App/spatiand-wpe/out/android}
    ndk=$work/chromium/src/third_party/android_toolchain/ndk/toolchains/llvm/prebuilt/linux-x86_64/sysroot
    sysroot=$work/sysroot-android
    rm -rf "$sysroot"; mkdir -p "$sysroot/usr/include" "$sysroot/usr/lib/pkgconfig"
    cp -a "$ndk/usr/include/." "$sysroot/usr/include/"
    cp -a "$ndk/usr/include/aarch64-linux-android/." "$sysroot/usr/include/"
    cp -a "$ndk/usr/lib/aarch64-linux-android/24/." "$sysroot/usr/lib/"
    for f in libc++_shared.so libc++_static.a libc++abi.a; do cp -a "$ndk/usr/lib/aarch64-linux-android/$f" "$sysroot/usr/lib/"; done
    cp -a "$deps/include/xkbcommon" "$sysroot/usr/include/"
    cp -a "$deps"/lib/libxkbcommon.so* "$sysroot/usr/lib/"
    sed 's|^prefix=.*|prefix=/usr|' "$deps/lib/pkgconfig/xkbcommon.pc" > "$sysroot/usr/lib/pkgconfig/xkbcommon.pc"
    # libffi, for Wayland's library: a Linux build links the system's as ffi_pic. The WPE build's is
    # a shared library (libffi.so, which the APK carries), linked under that name.
    cp -a "$deps/include/ffi.h" "$deps/include/ffitarget.h" "$sysroot/usr/include/"
    cp -a "$deps/lib/libffi.so" "$sysroot/usr/lib/"
    ln -sf libffi.so "$sysroot/usr/lib/libffi_pic.so"
    # The unwinder and atomics Android's compiler brings with it, which Chromium's clang does not: the
    # NDK's (r28c, the one Spatiand's Android builds use).
    ndk_full=${SPATIAND_NDK:-/media/shared/Linux/App/android-ndk-r28c}
    cp -a "$ndk_full"/toolchains/llvm/prebuilt/linux-x86_64/lib/clang/*/lib/linux/aarch64/libunwind.a \
          "$ndk_full"/toolchains/llvm/prebuilt/linux-x86_64/lib/clang/*/lib/linux/aarch64/libatomic.a "$sysroot/usr/lib/"
    # NSS, NSPR and libdbus, made for Android here (android-libc/deps.sh): certificates and D-Bus as on
    # the Deck, rather than Chromium without them, which upstream never builds for Linux.
    "$here/android-libc/deps.sh" "$work/android-deps"
    adeps=$work/android-deps
    cp -a "$adeps/include/." "$sysroot/usr/include/"
    cp -a "$adeps"/lib/*.so "$adeps/lib/libdbus-1.a" "$adeps/lib/dbus-1.0" "$sysroot/usr/lib/"
    for pc in nss nspr dbus-1; do
        sed 's|^prefix=.*|prefix=/usr|' "$adeps/lib/pkgconfig/$pc.pc" > "$sysroot/usr/lib/pkgconfig/$pc.pc"
    done
    # Mesa's description, which the GPU process reads for where to preload its drivers from: Android
    # has no Mesa, and its own drivers are loaded by its own libEGL.
    printf 'dridriverdir=/vendor/lib64/egl\nName: dri\nDescription: none here\nVersion: 0\n' > "$sysroot/usr/lib/pkgconfig/dri.pc"
    # What a Linux program links that Android's libc already is (Termux's way).
    echo "INPUT(-llog)" > "$sysroot/usr/lib/librt.so"
    printf '!<arch>\n' > "$sysroot/usr/lib/libpthread.a"
    printf '!<arch>\n' > "$sysroot/usr/lib/libresolv.a"
    # The programs built and run here during the build, and V8's snapshot maker: this machine's,
    # by Chromium's clang against Chromium's Debian sysroot (Termux's host toolchain).
    mkdir -p "$work/custom-toolchain"
    cat > "$work/custom-toolchain/BUILD.gn" <<'GN'
import("//build/config/sysroot.gni")
import("//build/toolchain/gcc_toolchain.gni")

gcc_toolchain("host") {
  cc = "/cr/chromium/src/third_party/llvm-build/Release+Asserts/bin/clang"
  cxx = "/cr/chromium/src/third_party/llvm-build/Release+Asserts/bin/clang++"
  ld = cxx
  ar = "/cr/chromium/src/third_party/llvm-build/Release+Asserts/bin/llvm-ar"
  nm = "/cr/chromium/src/third_party/llvm-build/Release+Asserts/bin/llvm-nm"
  toolchain_args = {
    current_os = "linux"
    current_cpu = "x64"
    v8_current_cpu = "arm64"
    is_clang = true
    sysroot = "/cr/chromium/src/build/linux/debian_bullseye_amd64-sysroot"
  }
}
GN
    inside bash -s <<'EOF'
set -e
cd /cr/chromium/src
# Chromium's FFmpeg for the system's, as for Linux: Spatiand's.
grep -q spatiand_ffmpeg third_party/ffmpeg/BUILD.gn || python3 build/linux/unbundle/replace_gn_files.py --system-libraries ffmpeg
# Cromite's arguments, its Linux ones changed in place for arm64 on Android's libc (GN refuses a
# value set twice): no sysroot of Debian's, no PGO (there is no profile for this), and no software
# AV1, as on the Deck -- nor any AV1 until Android's decoder is wired in.
ARGS=$(sed -e 's/^  target_cpu = "x64"/  target_cpu = "arm64"/' \
           -e 's/^  use_sysroot = true/  use_sysroot = false/' \
           -e 's/^  chrome_pgo_phase = 2 # enable pgo/  chrome_pgo_phase = 0/' \
           -e 's/^enable_dav1d_decoder=true/enable_dav1d_decoder=false/' \
           -e 's/^enable_av1_decoder=true/enable_av1_decoder=false/' \
           /cr/cromite/build/cromite.gn_args)
CLANG=/cr/chromium/src/third_party/llvm-build/Release+Asserts
mkdir -p out/android
cat > out/android/args.gn <<GN
target_os = "linux"
$ARGS
# Android's libc (android-libc/), as Termux builds it.
spatiand_android_libc = true
target_sysroot = "/cr/sysroot-android"
host_toolchain = "/cr/custom-toolchain:host"
v8_snapshot_toolchain = "/cr/custom-toolchain:host"
custom_toolchain_clang_base_path = "$CLANG"
custom_toolchain_clang_version = "$(ls $CLANG/lib/clang | head -1)"
custom_target_rust_abi_target = "aarch64-linux-android"
clang_use_chrome_plugins = false
treat_warnings_as_errors = false
use_custom_libcxx = false
use_custom_libcxx_for_host = true
use_clang_modules = false
use_allocator_shim = false
use_partition_alloc_as_malloc = false
enable_backup_ref_ptr_support = false
enable_backup_ref_ptr_slow_checks = false
enable_dangling_raw_ptr_checks = false
enable_dangling_raw_ptr_feature_flag = false
backup_ref_ptr_extra_oob_checks = false
enable_pointer_compression_support = false
is_cfi = false
use_cfi_icall = false
use_thin_lto = false
exclude_unwind_tables = false
clang_warning_suppression_file = ""
# A Wayland window of the room and nothing else of a Linux desktop: no X11, GTK, Pango, BlueZ, GLib,
# CUPS, Kerberos, PulseAudio or udev here (as Chromium's own headless build leaves them out). D-Bus
# and NSS as on the Deck (deps.sh). Fonts by Chromium's own fontconfig.
use_ozone = true
ozone_auto_platforms = false
ozone_platform = "wayland"
ozone_platform_wayland = true
ozone_platform_x11 = false
ozone_platform_headless = true
# Chromium's own minigbm: Ozone's Wayland is built with it, though nothing here may open a GPU
# device for it, and pictures go as shared memory.
use_system_minigbm = false
use_gtk = false
use_pangocairo = false
use_bluez = false
use_glib = false
use_gio = false
use_cups = false
use_kerberos = false
use_pulseaudio = false
use_alsa = false
rtc_use_pipewire = false
use_udev = false
use_vaapi = false
use_qt5 = false
use_qt6 = false
use_libpci = false
use_bundled_fontconfig = true
use_system_freetype = false
# Crash reports sent as on Android, by crashpad's own socket and BoringSSL: no libcurl here.
crashpad_http_transport_impl = "socket"
angle_enable_vulkan = true
angle_enable_swiftshader = true
angle_enable_vulkan_validation_layers = false
dawn_enable_vulkan_validation_layers = false
GN
gn gen out/android
# The machine also serves spatiand-host: the lowest priority. Everything that fails, not only the first.
nice -n 19 ionice -c 3 python3 /cr/depot_tools/siso.py ninja -C out/android chrome --offline -k 0
cp /cr/cromite/build/RELEASE out/android/
EOF
}

build() {
    [ "$target" = android ] && { build_android; return; }
    # Spatiand's own FFmpeg (third_party/ffmpeg, `build.sh linux` -- 9.0.2, one copy for every
    # program of Spatiand's): its headers and libraries where build/linux/unbundle/ffmpeg.gn
    # (patch 0003) looks for them. libsw* too: libavcodec needs libswresample to load.
    ffmpeg=${SPATIAND_FFMPEG:-$here/../ffmpeg/out/linux}
    staged=$work/chromium/src/third_party/spatiand_ffmpeg
    rm -rf "$staged"; mkdir -p "$staged/lib"
    cp -a "$ffmpeg/include" "$staged/"
    cp -a "$ffmpeg"/lib/libav*.so* "$ffmpeg"/lib/libsw*.so* "$staged/lib/"
    inside bash -s <<'EOF'
set -e
cd /cr/chromium/src
# Programs built here and run during the build (code_cache_generator) find Spatiand's FFmpeg in
# $ORIGIN/ffmpeg, one of the places patch 0003 has every program look.
mkdir -p out/deck
ln -sfn ../../third_party/spatiand_ffmpeg/lib out/deck/ffmpeg
# Chromium's own way to build on an FFmpeg of the system's: //third_party/ffmpeg becomes the shim.
grep -q spatiand_ffmpeg third_party/ffmpeg/BUILD.gn || python3 build/linux/unbundle/replace_gn_files.py --system-libraries ffmpeg
# Cromite's Linux arguments; Spatiand's changes made in place (GN refuses a value set twice):
# no software AV1 (the owner's choice) -- AV1 is read and offered, and decoded only on the GPU by
# Spatiand's FFmpeg (patch 0006; the Deck's GPU decodes AV1).
ARGS=$(sed -e "s/^enable_dav1d_decoder=true/enable_dav1d_decoder=false/" /cr/cromite/build/cromite.gn_args)
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
    [ "$target" = android ] && { O=$work/chromium/src/out/android; P=$work/package/spatimesh-android; }
    rm -rf "$P"; mkdir -p "$P"
    # What chrome/installer/linux/BUILD.gn packages, with every locale.
    # The program is named as what it is: SpatiKey names who is asking from what is running.
    cp -a "$O/chrome" "$P/spatimesh"
    for f in chrome_crashpad_handler libEGL.so libGLESv2.so libvulkan.so.1 libvk_swiftshader.so \
        vk_swiftshader_icd.json libqt5_shim.so libqt6_shim.so icudtl.dat snapshot_blob.bin v8_context_snapshot.bin \
        RELEASE xdg-mime xdg-settings; do
        [ -e "$O/$f" ] && cp -a "$O/$f" "$P/"
    done
    cp -a "$O"/*.pak "$P/"
    for d in locales MEIPreload PrivacySandboxAttestationsPreloaded; do [ -d "$O/$d" ] && cp -a "$O/$d" "$P/"; done
    cp "$work/chromium/src/chrome/app/theme/chromium/product_logo_256.png" "$P/icon.png"
    cp "$here/run.sh" "$P/run.sh"
    tar -C "$work/package" -czf "$P.tar.gz" "$(basename "$P")"
    echo "packaged $(du -sh "$P" | cut -f1) in $P.tar.gz"
}

mkdir -p "$work/spatiand-patches"
cp "$here"/patches/*.patch "$work/spatiand-patches/"
rm -rf "$work/android-libc"; cp -a "$here/android-libc" "$work/android-libc"
case $step in
    fetch) fetch ;;
    patch) patch_tree ;;
    build) build ;;
    package) package ;;
    all) fetch; patch_tree; build; package ;;
    *) echo "unknown step: $step" >&2; exit 2 ;;
esac
