#!/bin/sh
# A display standing in for the glasses on the Beam Pro (StandIn.java), for testing the room
# there with none plugged in: the room draws its world into it, in one eye, and its windows get
# their frames as on the glasses. The app must be one that takes it (MainActivity.glassesDisplay).
#
#   tools/beam-stand-in/run.sh start [width height]   make it (1920x1080 unless told)
#   tools/beam-stand-in/run.sh shot [file.png]        what it shows now, onto this machine
#   tools/beam-stand-in/run.sh stop                   take it away
#
# Its log, frames per 5 s among it, is /data/local/tmp/standin.log on the phone.
set -eu
# Where a picture asked for goes, said before this script moves to its own folder.
to=$(cd "$(dirname "${2:-.}")" 2>/dev/null && pwd)/$(basename "${2:-standin.png}")
cd "$(dirname "$0")"
SDK=${ANDROID_HOME:-$(case "$(uname -s)" in (Darwin) echo "$HOME/Library/Android/sdk" ;; (*) echo "$HOME/Android/Sdk" ;; esac)}
ADB="adb ${ANDROID_SERIAL:+-s $ANDROID_SERIAL}"
# The stand-in's display woken, which lights the phone's lock screen too: Android wakes both.
wake() {
    id=$($ADB shell cat /data/local/tmp/standin.log | sed -n 's/.*: display \([0-9]*\),.*/\1/p' | head -1)
    [ -n "$id" ] && $ADB shell input -d "$id" keyevent KEYCODE_WAKEUP
}
case "${1:-}" in
start)
    out=$(mktemp -d)
    javac --release 17 -Xlint:-options -classpath "$SDK/platforms/android-34/android.jar" -d "$out" StandIn.java
    "$SDK/build-tools/36.0.0/d8" --min-api 34 --lib "$SDK/platforms/android-34/android.jar" --output "$out" "$out"/id/prasetya/spatiand/standin/*.class
    $ADB push "$out/classes.dex" /data/local/tmp/standin.dex >/dev/null
    rm -rf "$out"
    # Apart: a shell whose own command names the program would be killed with it.
    $ADB shell 'pkill -f "[s]tandin.StandIn"' || true
    $ADB shell "CLASSPATH=/data/local/tmp/standin.dex nohup app_process / id.prasetya.spatiand.standin.StandIn ${2:-1920} ${3:-1080} </dev/null > /data/local/tmp/standin.log 2>&1 &"
    sleep 2
    $ADB shell cat /data/local/tmp/standin.log
    wake
    ;;
shot)
    # Its display group sleeps with a locked phone, within the minute: the room goes on
    # presenting into it (its log's frame counts hold) but the picture is black. Woken first.
    wake
    sleep 1.5
    $ADB shell "rm -f /data/local/tmp/standin.png; touch /data/local/tmp/standin.shot"
    for _ in 1 2 3 4 5 6 7 8 9 10; do
        $ADB shell "[ -f /data/local/tmp/standin.png ]" && break
        sleep 0.5
    done
    [ "${2:-}" ] || to=$OLDPWD/standin.png
    $ADB pull /data/local/tmp/standin.png "$to" >/dev/null
    echo "$to"
    ;;
stop)
    $ADB shell 'pkill -f "[s]tandin.StandIn"' || true
    $ADB shell rm -f /data/local/tmp/standin.shot
    ;;
*)
    sed -n 2,10p "$0"
    exit 2
    ;;
esac
