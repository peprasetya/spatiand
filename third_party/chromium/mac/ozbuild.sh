#!/bin/bash
# The Mac's Linux-UI SpatiMesh build, on littleotter (docs/spatimesh-mac-port.md): copied to
# ~/Developer/spatiand-chromium-mac/ozbuild.sh there and run as
#   nohup caffeinate -i -s bash ozbuild.sh > logs/build-ozoneN.log 2>&1 &
# It goes on from what was built before, so a pass stopped by a shutdown is simply run again.
cd ~/Developer/spatiand-chromium-mac/chromium/src
export PATH="$HOME/Developer/spatiand-chromium-mac/depot_tools:$PWD/third_party/cpython3/host/bin:$PATH" DEPOT_TOOLS_UPDATE=0 DEPOT_TOOLS_METRICS=0
echo "start $(date)"
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
