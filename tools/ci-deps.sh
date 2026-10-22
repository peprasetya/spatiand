#!/bin/sh
# What a Linux machine without a desk needs to build Spatiand: the packages, Rust, and for the
# Beam Pro the Android SDK and NDK. One script for both places that start from nothing -- the
# CI workflow (.github/workflows/ci.yml) and the setup script of a Claude Code cloud
# environment -- so they cannot drift apart.
#
#   tools/ci-deps.sh linux             the Deck's compositor and every crate that builds on Linux
#   tools/ci-deps.sh ffmpeg            FFmpeg as Spatiand ships it (third_party/ffmpeg), which Ubuntu has not got
#   tools/ci-deps.sh android           the Beam Pro: Rust target, cargo-ndk, SDK, NDK, bison
#   tools/ci-deps.sh linux ffmpeg android     all of it
#
# Debian/Ubuntu (apt). Needs root, or sudo. The Mac cannot be built from Linux: see ci.yml.
# After it: `. ~/.spatiand-ci-env` for ANDROID_HOME, FFmpeg's PKG_CONFIG_PATH and friends.
set -eu

SUDO=""
[ "$(id -u)" = 0 ] || SUDO="sudo"
export DEBIAN_FRONTEND=noninteractive

# Versions the Android build is written for: android/build.sh.
BUILD_TOOLS=36.0.0
PLATFORM=android-34
NDK=28.2.13676358
CMDLINE_TOOLS=11076708

apt_update_once() { [ -n "${UPDATED:-}" ] || { $SUDO apt-get update -qq; UPDATED=1; }; }

rust() {
    command -v cargo >/dev/null 2>&1 && return 0
    curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal
    . "$HOME/.cargo/env"
}

linux() {
    apt_update_once
    # Smithay's backends (libseat, udev, libinput, GBM/DRM, EGL), the keyboard, Wayland,
    # PipeWire for sound capture, VA-API (spatiand-video links it), clang for bindgen, and
    # WebKitGTK with a display to run it on for SpatiMesh's engine on Linux (docs/spatimesh.md),
    # and Mesa's software GL and a font for the compositor to run SpatiMesh with no GPU.
    $SUDO apt-get install -y -qq --no-install-recommends \
        build-essential pkg-config clang libclang-dev curl ca-certificates git \
        libseat-dev libudev-dev libinput-dev libgbm-dev libdrm-dev \
        libegl1-mesa-dev libgles2-mesa-dev libxkbcommon-dev libwayland-dev \
        libpipewire-0.3-dev libspa-0.2-dev libasound2-dev libusb-1.0-0-dev libdisplay-info-dev libva-dev \
        libx11-dev libxcb1-dev libxcb-composite0-dev libxcb-xfixes0-dev libxcb-render0-dev \
        libxcb-shape0-dev libxcb-xkb-dev xwayland \
        libwebkit2gtk-4.1-dev xvfb xauth libgl1-mesa-dri fonts-dejavu-core
    rust
}

android() {
    apt_update_once
    $SUDO apt-get install -y -qq --no-install-recommends \
        build-essential pkg-config clang curl ca-certificates unzip git bison make openjdk-17-jdk-headless
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

    # The Android scripts take these from the environment (android/build.sh).
    ENV_LINES="${ENV_LINES}export ANDROID_HOME=\"$SDK\"
export ANDROID_NDK_HOME=\"$SDK/ndk/$NDK\"
export BISON=\"$(command -v bison)\"
export PATH=\"\$HOME/.cargo/bin:\$PATH\"
"
    echo "Android: . ~/.spatiand-ci-env first"
}

# FFmpeg: the version Spatiand ships everywhere (third_party/ffmpeg/versions.sh), built by the same
# script that builds it for the Deck, into third_party/ffmpeg/out/linux -- which CI keeps between runs.
ffmpeg() {
    apt_update_once
    $SUDO apt-get install -y -qq --no-install-recommends build-essential pkg-config nasm curl xz-utils \
        libopus-dev libssl-dev libva-dev libdrm-dev zlib1g-dev libbz2-dev
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
