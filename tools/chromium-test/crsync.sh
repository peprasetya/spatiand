#!/bin/bash
# SpatiMesh against a COPY of the shared folder, in the Deck's test room.
#   crsync.sh [fresh|keep] <seconds> [chrome args and URL]
# fresh: copy the real folder again, new browser profile. keep: both as the last run left them.
T=/tmp/crt; C=/tmp/crt-config; P=/tmp/crt-keep
mode=${1:-fresh}; secs=${2:-120}; shift 2
if [ "$mode" = fresh ]; then rm -rf $C $P; cp -a ~/.config/spatiand $C; mkdir -p $P; fi
rm -rf $T; mkdir -p $T
: > /tmp/crt-chrome.log
cd /home/deck/spatiand
export SPATIAND_APPS_CONFIG=$C
SPATIAND_BACKEND=snapshot SPATIAND_HMD=null SPATIAND_INPUT=off SPATIAND_XWAYLAND=off SPATIAND_SNAPSHOT_MODE=flat \
XDG_RUNTIME_DIR=$T SPATIAND_CLIENT_WAIT=40 \
SPATIAND_CLIENT="/home/deck/crt-wrap.sh --user-data-dir=$P --no-first-run --no-default-browser-check --remote-debugging-port=9333 ${*:-about:blank}" \
SPATIAND_STEPS="${STEPS:-wait:$secs;dump:$T/page.png}" SPATIAND_SNAPSHOT=$T/room.png timeout $((secs+60)) ./target/release/spatiand > $T/spatiand.log 2>&1
echo "spatiand exit=$?"
sleep 2; pkill -f "[u]ser-data-dir=$P" && echo "killed a leftover" || echo "no leftover"
