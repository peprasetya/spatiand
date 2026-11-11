#!/bin/sh
# go.sh <png> [wait seconds] [extra SpatiMesh flags and URLs...]: SpatiMesh in the snapshot room,
# started with the switches the room itself gives it (crates/spatiand/src/own.rs), drawn by the
# GPU as the installed one is. SOFT=1: software drawing. SOUND=1: the room's sound.
# SPATIAND_STEPS="click:u,v;wait:2;..." drives it (crates/spatiand/src/backend_snapshot.rs).
OUT=$1; WAIT=${2:-30}; shift 2
SRC=$HOME/Developer/spatiand-chromium-mac/chromium/src
L="$SRC/out/mac-ozone/SpatiMesh.app/Contents/Frameworks/SpatiMesh Framework.framework/Versions/153.0.8010.37/Libraries"
[ -e "$L/libavcodec.63.dylib" ] || cp -a $SRC/third_party/spatiand_ffmpeg/lib/*.dylib "$L/"
[ -e "$L/libssl.3.dylib" ] || cp -a $HOME/Applications/Spatiand.app/Contents/Frameworks/libssl.3.dylib $HOME/Applications/Spatiand.app/Contents/Frameworks/libcrypto.3.dylib "$L/"
mkdir -p /tmp/smtest/rt /tmp/smtest/apps; chmod 700 /tmp/smtest/rt
echo "${SOFT:+--disable-gpu} $@" > /tmp/smtest/flags
pkill -f "mac-ozone/SpatiMesh.app" 2>/dev/null
cd /tmp/smtest; rm -f "$OUT"
export XDG_RUNTIME_DIR=${XDG_RUNTIME_DIR:-/tmp/smtest/rt} SPATIAND_SNAPSHOT_SOUND=${SOUND:-0} RUST_LOG=info SPATIAND_BACKEND=snapshot SPATIAND_HMD=null SPATIAND_INPUT=off SPATIAND_XWAYLAND=off SPATIAND_SNAPSHOT="$OUT" SPATIAND_SNAPSHOT_MODE=flat SPATIAND_CLIENT_WAIT=$WAIT SPATIAND_CLIENT=/tmp/smtest/run.sh
timeout $((WAIT + 60)) $HOME/Developer/Spatiand/crates/spatiand-mac/target/release/spatiand-mac > /tmp/smtest/room.log 2>&1
pkill -f "mac-ozone/SpatiMesh.app" 2>/dev/null
grep -v "smithay\|tracing" /tmp/smtest/room.log | grep -i "exited\|drew\|wrote\|may need" | cut -c1-200
