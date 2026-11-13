#!/bin/bash
# The Mac's Linux-UI SpatiMesh build, on littleotter (docs/spatimesh-mac-port.md): copied to
# ~/Developer/spatimesh-mac/ozbuild.sh there and run as
#   nohup caffeinate -i -s bash ozbuild.sh > logs/build-ozoneN.log 2>&1 &
# It goes on from what was built before, so a pass stopped by a shutdown is simply run again.
cd ~/Developer/spatimesh-mac/chromium/src
export PATH="$HOME/Developer/spatimesh-mac/depot_tools:$PWD/third_party/cpython3/host/bin:$PATH" DEPOT_TOOLS_UPDATE=0 DEPOT_TOOLS_METRICS=0
echo "start $(date)"
# The keyboard's xkbcommon: the room's own (mac/xkbcommon/build.sh), linked into SpatiMesh as the
# Beam Pro links the compositor's (its headers are kept in the tree: mac/wip).
X=third_party/spatiand_xkbcommon/lib
mkdir -p "$X"
[ -f "$X/libxkbcommon.a" ] || lipo -thin arm64 "$HOME/Developer/Spatiand/mac/xkbcommon/out/libxkbcommon.a" -output "$X/libxkbcommon.a"
nice -n 10 python3 ../../depot_tools/siso.py ninja -C out/mac-ozone -k 0 chrome --offline
# Spatiand's FFmpeg and the OpenSSL it links, in the framework's Libraries, where its rpath looks
# (mac/0001). The bundle step makes that folder again on every build.
L="out/mac-ozone/SpatiMesh.app/Contents/Frameworks/SpatiMesh Framework.framework/Versions/Current/Libraries"
if [ -d "$L" ]; then
  cp -a third_party/spatiand_ffmpeg/lib/*.dylib "$L/"
  cp -a "$HOME/Applications/Spatiand.app/Contents/Frameworks/libssl.3.dylib" \
        "$HOME/Applications/Spatiand.app/Contents/Frameworks/libcrypto.3.dylib" "$L/"
fi
echo "end $(date)"
