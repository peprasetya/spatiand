#!/bin/sh
# Build the measured head the Mac app carries: libmysofa (which reads SOFA files) and the dataset it
# ships, a measurement of a dummy head's ears all round a sphere. The Deck gets both from SteamOS;
# a Mac has neither, so the app brings its own. Needs cmake and a network, once.
#
# Upstream's own release, the same tag and SHA-256 as Android's (android/mysofa/build.sh): the
# same libmysofa on every platform, not whatever its master is that day.
#
# Writes mac/Resources/hrtf/{libmysofa.1.dylib,default.sofa,NOTICE.txt}; make-app.sh puts them in the app.
set -e
cd "$(dirname "$0")/.."
root=$(pwd)
out=Resources/hrtf
if [ -f "$out/libmysofa.1.dylib" ] && [ -f "$out/default.sofa" ]; then exit 0; fi
VERSION=1.3.5
SHA=f29508c335c83d8703f943ffc9ca783ac39aca84e851357f13a55af0f8143137
work=$(mktemp -d)
curl -sSfL -o "$work/libmysofa.tar.gz" "https://github.com/hoene/libmysofa/archive/refs/tags/v$VERSION.tar.gz"
echo "$SHA  $work/libmysofa.tar.gz" | shasum -a 256 -c - >/dev/null || { echo "libmysofa: checksum mismatch"; rm -rf "$work"; exit 1; }
tar -C "$work" -xzf "$work/libmysofa.tar.gz"
mv "$work/libmysofa-$VERSION" "$work/libmysofa"
cd "$work/libmysofa"
mkdir b && cd b
cmake .. -DBUILD_SHARED_LIBS=ON -DBUILD_STATIC_LIBS=OFF -DBUILD_TESTS=OFF -DCMAKE_BUILD_TYPE=Release \
      -DCMAKE_OSX_DEPLOYMENT_TARGET=14.0 >/dev/null
make -j8 >/dev/null
here="$root"
mkdir -p "$here/$out"
cp "$work/libmysofa/b/src/libmysofa.1.dylib" "$here/$out/libmysofa.1.dylib" 2>/dev/null \
  || cp -L "$work/libmysofa/b/src/libmysofa.1.dylib" "$here/$out/libmysofa.1.dylib"
cp "$work/libmysofa/share/default.sofa" "$here/$out/default.sofa"
cat > "$here/$out/NOTICE.txt" <<NOTICE
libmysofa $VERSION, https://github.com/hoene/libmysofa/releases/tag/v$VERSION
Copyright (c) 2016-2017, Symonics GmbH, Christian Hoene. BSD 3-clause licence; its text follows.

$(cat "$work/libmysofa/LICENSE")

default.sofa is the "MIT KEMAR normal pinna" measurement of a KEMAR dummy head, made at the MIT Media
Lab by Bill Gardner and Keith Martin, and shipped with libmysofa.
NOTICE
rm -rf "$work"
echo "built the measured head in $out"
