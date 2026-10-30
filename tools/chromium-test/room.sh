#!/bin/bash
# Starts the test room so that it outlives this ssh session: room.sh <keep|fresh> <seconds> [args]
systemctl --user stop crt-room 2>/dev/null; systemctl --user reset-failed crt-room 2>/dev/null
pkill -f "[u]ser-data-dir=/tmp/crt"; sleep 1
M=$1; S=$2; shift 2
systemd-run --user --quiet --unit=crt-room --collect --setenv=STEPS="${STEPS:-wait:$S;dump:/tmp/crt/page.png}" --setenv=HOME=$HOME --setenv=XDG_RUNTIME_DIR_REAL=$XDG_RUNTIME_DIR bash -c "$HOME/crsync.sh $M $S $* > /tmp/crsync.out 2>&1"
