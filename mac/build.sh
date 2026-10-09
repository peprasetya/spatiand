#!/bin/sh
# Build the Rust half, then the app. Run from anywhere. The first time, this also fetches what the
# compositor needs that macOS has not got: libxkbcommon (built here) and ANGLE (taken from an
# installed Electron application; see fetch-angle.sh).
set -e
cd "$(dirname "$0")"
[ -f xkbcommon/out/libxkbcommon.a ] || xkbcommon/build.sh
[ -f vendor/angle/libEGL.dylib ] || ./fetch-angle.sh
(cd ../crates/spatiand-mac && MACOSX_DEPLOYMENT_TARGET=14.0 cargo build)
# The library is a file SwiftPM does not watch: without this it links the old one.
rm -f .build/debug/Spatiand
swift build
