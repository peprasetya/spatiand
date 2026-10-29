#!/usr/bin/env bash
# Build SpatiKey's browser extensions into dist/ (docs/spatikey.md, "The extensions"):
#
#   dist/spatikey-chrome/     Manifest V3 for Chrome, Chromium, Brave and Edge: "Load unpacked"
#   dist/spatikey-firefox/    Manifest V3 for Firefox 128 and later: a temporary add-on
#   dist/spatikey-firefox.xpi the same, zipped, for Firefox Developer Edition with signatures off
#
#   extensions/spatikey/build.sh [folder]
#
# Given a folder, it builds there instead of dist/ (the tests build into folders of their own, so
# that they can run at the same time).
#
# The same four scripts go into both; only the manifest differs (Chrome's background is a
# service worker, Firefox's a script; Firefox's id is spatikey@prasetya.id, Chrome's comes from
# the manifest's fixed key). Nothing is fetched and nothing needs installing.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
DIST="${1:-$HERE/dist}"
mkdir -p "$DIST"
DIST="$(cd "$DIST" && pwd)"
# Bumped with every change that browsers should see as a new version.
VERSION=0.1.0
SCRIPTS=(webauthn.js forms.js content.js background.js)
ICONS=(key-16.png key-32.png key-48.png key-128.png)

for flavor in chrome firefox; do
    out="$DIST/spatikey-$flavor"
    rm -rf "$out"
    mkdir -p "$out/icons"
    for script in "${SCRIPTS[@]}"; do
        cp "$HERE/src/$script" "$out/$script"
    done
    for icon in "${ICONS[@]}"; do
        cp "$HERE/icons/$icon" "$out/icons/$icon"
    done
    sed "s/@VERSION@/$VERSION/" "$HERE/manifest.$flavor.json" > "$out/manifest.json"
    echo "built $out"
done

# Firefox installs an add-on for good only from a file; Developer Edition takes an unsigned one
# once xpinstall.signatures.required is off.
rm -f "$DIST/spatikey-firefox.xpi"
if command -v zip >/dev/null; then
    (cd "$DIST/spatikey-firefox" && zip -q -r -X "$DIST/spatikey-firefox.xpi" .)
    echo "built $DIST/spatikey-firefox.xpi"
else
    echo "zip is not installed: no .xpi (the folder still loads as a temporary add-on)" >&2
fi
