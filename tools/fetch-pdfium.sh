#!/bin/sh
# PDFium, which SpatiLoupe draws PDFs with (docs/spatiand-apps.md): Chromium's own PDF engine,
# as built by bblanchon/pdfium-binaries. It is loaded at run time from beside the programs, so
# it is fetched, not built, and not kept in the history.
#
#   tools/fetch-pdfium.sh [mac-x64|mac-arm64|linux-x64|android-arm64 ...]
#
# Each lands in third_party/pdfium/<platform>/lib. With no argument, the ones Spatiand ships for.
set -eu
VERSION=chromium/8100
cd "$(dirname "$0")/.."
[ $# -gt 0 ] || set -- mac-x64 mac-arm64 linux-x64 android-arm64
for platform in "$@"; do
    dir=third_party/pdfium/$platform
    if [ -f "$dir/.version" ] && [ "$(cat "$dir/.version")" = "$VERSION" ]; then
        continue
    fi
    rm -rf "$dir"
    mkdir -p "$dir"
    curl -fsSL "https://github.com/bblanchon/pdfium-binaries/releases/download/$VERSION/pdfium-$platform.tgz" | tar -xz -C "$dir"
    echo "$VERSION" > "$dir/.version"
    echo "pdfium $VERSION for $platform in $dir"
done
