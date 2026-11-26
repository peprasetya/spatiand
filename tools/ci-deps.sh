#!/bin/sh
# What a Linux machine without a desk needs to build Spatiand: the packages, Rust, and for the
# Beam Pro the Android SDK and NDK. One script for both places that start from nothing -- the
# CI workflow (.github/workflows/ci.yml) and the setup script of a Claude Code cloud
# environment -- so they cannot drift apart.
#
#   tools/ci-deps.sh linux             the Deck's compositor and every crate that builds on Linux
#   tools/ci-deps.sh ffmpeg            FFmpeg as Spatiand ships it (third_party/ffmpeg), which Ubuntu has not got
#   tools/ci-deps.sh android           the Beam Pro: Rust target, cargo-ndk, SDK, NDK
#   tools/ci-deps.sh linux ffmpeg android     all of it
#
# Debian/Ubuntu (apt). Needs root, or sudo. The Mac cannot be built from Linux: see ci.yml.
# After it: `. ~/.spatiand-ci-env` for ANDROID_HOME, FFmpeg's PKG_CONFIG_PATH and friends.
set -eu

SUDO=""
[ "$(id -u)" = 0 ] || SUDO="sudo"
export DEBIAN_FRONTEND=noninteractive

# Versions the Android build is written for: android/build.sh.
BUILD_TOOLS=37.0.0
PLATFORM=android-37.2
# The NDK: the one android/ndk.sh names, where every Android build reads it.
NDK=$(sed -n 's/^SPATIAND_NDK_VERSION=\([^ ]*\).*/\1/p' "$(dirname "$0")/../android/ndk.sh")
CMDLINE_TOOLS=16111833

apt_update_once() { [ -n "${UPDATED:-}" ] || { $SUDO apt-get update -qq; UPDATED=1; }; }

rust() {
    command -v cargo >/dev/null 2>&1 && return 0
    curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal
    . "$HOME/.cargo/env"
}

linux() {
    apt_update_once
    # Smithay's backends (libseat, udev, libinput, GBM/DRM, EGL), the keyboard, Wayland,
    # PipeWire for sound capture, VA-API (spatiand-video links it), clang for bindgen, and Mesa's
    # software GL and a font for the compositor to run its applications with no GPU.
    $SUDO apt-get install -y -qq --no-install-recommends \
        build-essential pkg-config clang libclang-dev curl ca-certificates git \
        libseat-dev libudev-dev libinput-dev libgbm-dev libdrm-dev \
        libegl1-mesa-dev libgles2-mesa-dev libxkbcommon-dev libwayland-dev \
        libpipewire-0.3-dev libspa-0.2-dev libasound2-dev libusb-1.0-0-dev libdisplay-info-dev libva-dev \
        libx11-dev libxcb1-dev libxcb-composite0-dev libxcb-xfixes0-dev libxcb-render0-dev \
        libxcb-shape0-dev libxcb-xkb-dev xwayland \
        libgl1-mesa-dri fonts-dejavu-core
    rust
}

android() {
    apt_update_once
    # CMake for the BoringSSL that android/build.sh builds into FFmpeg for Android.
    $SUDO apt-get install -y -qq --no-install-recommends \
        build-essential pkg-config clang curl ca-certificates unzip git make cmake openjdk-17-jdk-headless
    boringssl
    rust
    rustup target add aarch64-linux-android
    command -v cargo-ndk >/dev/null 2>&1 || cargo install cargo-ndk --locked

    SDK="${ANDROID_HOME:-$HOME/android-sdk}"
    if [ ! -x "$SDK/cmdline-tools/latest/bin/sdkmanager" ] && ! command -v sdkmanager >/dev/null 2>&1; then
        mkdir -p "$SDK/cmdline-tools"
        curl -fsSL -o /tmp/cmdline-tools.zip \
            "https://dl.google.com/android/repository/commandlinetools-linux-${CMDLINE_TOOLS}_latest.zip"
        unzip -q -o /tmp/cmdline-tools.zip -d "$SDK/cmdline-tools"
        rm -rf "$SDK/cmdline-tools/latest"
        mv "$SDK/cmdline-tools/cmdline-tools" "$SDK/cmdline-tools/latest"
    fi
    SDKMANAGER="$SDK/cmdline-tools/latest/bin/sdkmanager"
    [ -x "$SDKMANAGER" ] || SDKMANAGER="$(command -v sdkmanager)"
    yes | "$SDKMANAGER" --sdk_root="$SDK" --licenses >/dev/null 2>&1 || true
    "$SDKMANAGER" --sdk_root="$SDK" "platforms;$PLATFORM" "build-tools;$BUILD_TOOLS" "ndk;$NDK" >/dev/null

    # The Android scripts take these from the environment (android/build.sh, android/ndk.sh);
    # ANDROID_NDK_HOME, the same NDK, for cargo-ndk run on its own (ci.yml).
    ENV_LINES="${ENV_LINES}export ANDROID_HOME=\"$SDK\"
export SPATIAND_NDK=\"$SDK/ndk/$NDK\"
export ANDROID_NDK_HOME=\"$SDK/ndk/$NDK\"
export PATH=\"\$HOME/.cargo/bin:\$PATH\"
"
    echo "Android: . ~/.spatiand-ci-env first"
}

# BoringSSL, which FFmpeg's https is built with: SpatiMesh's own, read from the source of its
# Chromium (third_party/ffmpeg/versions.sh). Where there is no Chromium tree, as here, the same
# source: the revision named by the DEPS of the Chromium third_party/chromium/build.sh pins, with
# SpatiMesh's own patch to it (third_party/chromium/patches/sub/boringssl) committed on it. Dated
# as the patch is dated, so that every run makes the same commit, which FFmpeg's stamp names (and
# a cached FFmpeg is still the one it asks for); for Chromium 157 that is the very commit of
# deepmagpie's tree.
boringssl() {
    [ -z "${BORINGSSL_FETCHED:-}" ] || return 0
    BORINGSSL_FETCHED=1
    . third_party/ffmpeg/versions.sh
    [ -d "$BORINGSSL_SRC" ] && return 0
    chromium=$(sed -n 's/^CHROMIUM_VERSION=\([^ ]*\).*/\1/p' third_party/chromium/build.sh)
    rev=$(curl -fsSL "https://chromium.googlesource.com/chromium/src/+/refs/tags/$chromium/DEPS?format=TEXT" |
        base64 -d | sed -n "s/^ *'boringssl_revision': '\([0-9a-f]*\)',.*/\1/p")
    [ -n "$rev" ] || { echo "no boringssl_revision in the DEPS of Chromium $chromium" >&2; exit 1; }
    dir="$HOME/boringssl-$rev"
    if [ ! -d "$dir/.git" ]; then
        git init -q "$dir"
        git -C "$dir" fetch -q --depth 1 https://boringssl.googlesource.com/boringssl "$rev"
        git -C "$dir" checkout -q FETCH_HEAD
        for p in "$(pwd)"/third_party/chromium/patches/sub/boringssl/*.patch; do
            git -C "$dir" -c user.name="Spatiand build" -c user.email=spatiand@localhost am -q --committer-date-is-author-date "$p"
        done
    fi
    export SPATIAND_BORINGSSL="$dir"
    ENV_LINES="${ENV_LINES}export SPATIAND_BORINGSSL=\"$dir\"
"
}

# FFmpeg: the version Spatiand ships everywhere (third_party/ffmpeg/versions.sh), built by the same
# script that builds it for the Deck, into third_party/ffmpeg/out/linux -- which CI keeps between runs.
# Its https is the BoringSSL above, built with CMake, inside it: no OpenSSL of the system's.
ffmpeg() {
    apt_update_once
    $SUDO apt-get install -y -qq --no-install-recommends build-essential pkg-config nasm curl xz-utils \
        git cmake ninja-build libopus-dev libva-dev libdrm-dev zlib1g-dev libbz2-dev
    boringssl
    third_party/ffmpeg/build.sh linux
    prefix="$(pwd)/third_party/ffmpeg/out/linux"
    ENV_LINES="${ENV_LINES}export FFMPEG_PKG_CONFIG_PATH=\"$prefix/lib/pkgconfig\"
export PKG_CONFIG_PATH=\"$prefix/lib/pkgconfig\${PKG_CONFIG_PATH:+:\$PKG_CONFIG_PATH}\"
export LD_LIBRARY_PATH=\"$prefix/lib\${LD_LIBRARY_PATH:+:\$LD_LIBRARY_PATH}\"
"
    echo "FFmpeg in $prefix: . ~/.spatiand-ci-env first"
}

[ $# -gt 0 ] || { sed -n '2,13p' "$0"; exit 1; }
ENV_LINES=""
for target in "$@"; do
    case "$target" in
        linux) linux ;;
        ffmpeg) ffmpeg ;;
        android) android ;;
        *) echo "unknown target: $target" >&2; exit 1 ;;
    esac
done
# What the targets asked for, written once: the file is what this run set up, no more.
[ -z "$ENV_LINES" ] || printf '%s' "$ENV_LINES" > "$HOME/.spatiand-ci-env"
