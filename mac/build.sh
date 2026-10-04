#!/bin/sh
# Build the Rust link, then the app. Run from anywhere.
set -e
cd "$(dirname "$0")"
MACOSX_DEPLOYMENT_TARGET=14.0 cargo build -p spatiand-mac-core --manifest-path ../Cargo.toml
swift build
