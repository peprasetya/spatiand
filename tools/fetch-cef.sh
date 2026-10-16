#!/bin/sh
# CEF's own build of the version Steam's Chromium is (docs/spatimesh.md), for building and
# testing spatimesh-cef where there is no Steam: its headers (CEF_ROOT=folder), and in
# folder/steam its library laid out as Steam lays out its own in ubuntu12_64 -- libcef.so beside
# everything it reads (SPATIMESH_CEF_DIR=folder/steam).
#
#   tools/fetch-cef.sh [folder]     ~/cef if not given
#
# On SteamOS SpatiMesh uses Steam's own Chromium; this is for CI, and for building.
set -eu
OUT=${1:-$HOME/cef}
CHROMIUM=126.0.6478.183
SITE=https://cef-builds.spotifycdn.com
if [ -f "$OUT/include/capi/cef_app_capi.h" ] && [ -f "$OUT/steam/libcef.so" ]; then
    echo "CEF already in $OUT"
    exit 0
fi
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
curl -sSfL -o "$work/index.json" "$SITE/index.json"
# The newest of CEF's builds of that Chromium, minimal (no samples, no debug build).
python3 -I - "$work/index.json" "$CHROMIUM" > "$work/pick" <<'PY'
import json, sys
index = json.load(open(sys.argv[1]))
for version in index["linux64"]["versions"]:
    if version.get("chromium_version") == sys.argv[2]:
        for f in version["files"]:
            if f.get("type") == "minimal":
                print(f["name"], f["sha1"])
                sys.exit(0)
sys.exit("no CEF build of Chromium %s for linux64 in CEF's index" % sys.argv[2])
PY
read -r name sha1 < "$work/pick"
curl -sSfL -o "$work/cef.tar.bz2" "$SITE/$(printf %s "$name" | sed 's/+/%2B/g')"
echo "$sha1  $work/cef.tar.bz2" | sha1sum -c - >/dev/null
mkdir -p "$OUT"
tar -xjf "$work/cef.tar.bz2" -C "$OUT" --strip-components=1
# Steam's layout: one folder with the library and its resources, as ubuntu12_64 has them.
rm -rf "$OUT/steam"
mv "$OUT/Release" "$OUT/steam"
cp -a "$OUT/Resources/." "$OUT/steam/"
rm -rf "$OUT/Resources"
echo "CEF ${name%.tar.bz2} in $OUT, its library as Steam lays it out in $OUT/steam"
