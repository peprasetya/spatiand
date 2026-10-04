#!/bin/sh
# Build the measured head the Mac app carries: libmysofa (which reads SOFA files) and the dataset it
# ships, a measurement of a dummy head's ears all round a sphere. The Deck gets both from SteamOS;
# a Mac has neither, so the app brings its own. Needs git, cmake and a network, once.
#
# Writes mac/Resources/hrtf/{libmysofa.1.dylib,default.sofa,NOTICE.txt}; make-app.sh puts them in the app.
set -e
cd "$(dirname "$0")/.."
root=$(pwd)
out=Resources/hrtf
if [ -f "$out/libmysofa.1.dylib" ] && [ -f "$out/default.sofa" ]; then exit 0; fi
work=$(mktemp -d)
git clone --depth 1 https://github.com/hoene/libmysofa.git "$work/libmysofa" >/dev/null 2>&1
cd "$work/libmysofa"
commit=$(git rev-parse --short HEAD)
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
libmysofa (commit $commit), https://github.com/hoene/libmysofa
Copyright (c) 2016-2017, Symonics GmbH, Christian Hoene. BSD 3-clause licence; its text follows.

$(cat "$work/libmysofa/LICENSE")

default.sofa is the "MIT KEMAR normal pinna" measurement of a KEMAR dummy head, made at the MIT Media
Lab by Bill Gardner and Keith Martin, and shipped with libmysofa.
NOTICE
rm -rf "$work"
echo "built the measured head in $out"
