#!/bin/sh
# Spatiand's shell helper (android/src/id/prasetya/spatiand/ShellHelper.java) started by hand
# over adb from a computer: what Spatiand does itself through the phone's own Wireless
# debugging, for testing Android's apps in the room without it.
#
#   tools/beam-shell/run.sh start    start it from the installed APK; one running is replaced
#   tools/beam-shell/run.sh stop     stop it; the apps' displays go with it
#   tools/beam-shell/run.sh log      what it has said
#
# Its log is /data/local/tmp/spatiand-shell.log on the phone, and logcat's spatiand-shell tag.
set -eu
ADB="adb ${ANDROID_SERIAL:+-s $ANDROID_SERIAL}"
LOG=/data/local/tmp/spatiand-shell.log
case "${1:-}" in
start)
    apk=$($ADB shell pm path id.prasetya.spatiand | sed -n 's/^package://p' | head -1 | tr -d '\r')
    [ -n "$apk" ] || { echo "Spatiand is not installed"; exit 1; }
    # setsid: its own session, so the shell adb opened can end under it.
    $ADB shell "CLASSPATH=$apk setsid app_process /system/bin --nice-name=spatiand-shell \
        id.prasetya.spatiand.ShellHelper </dev/null >$LOG 2>&1 &"
    sleep 3
    $ADB shell cat $LOG
    ;;
stop)
    # By its name, never by its arguments: those name the app's own package. pidof, not pkill
    # -x: app_process names its main thread "main", which is what pkill matches.
    $ADB shell 'p=$(pidof spatiand-shell); [ -z "$p" ] || kill $p' || true
    ;;
log)
    $ADB shell cat $LOG
    ;;
*)
    sed -n 2,10p "$0"
    exit 2
    ;;
esac
