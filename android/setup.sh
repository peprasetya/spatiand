#!/bin/sh
# The one-time setup that makes the Beam Pro's glasses Spatiand's, from a computer with adb.
# Everything here survives reboots; none of it needs doing again.
#
#   android/setup.sh             with the glasses plugged into the Beam Pro
#   android/setup.sh nebula      give the glasses back to Nebula (undoes the first step)
#
# 1. Nebula disabled. XREAL's framework hands the glasses to Nebula whatever else is installed
#    (docs/beam-pro.md), so while it is enabled it takes them on every plug-in.
# 2. "Display over other apps" for Spatiand: its picture sits above XREAL's placeholder, and
#    it may open itself when the glasses are plugged in.
# 3. A persistent USB permission for the glasses: no question on plugging in, ever.
set -eu
cd "$(dirname "$0")"
PKG=id.prasetya.spatiand
ADB="adb ${ANDROID_SERIAL:+-s $ANDROID_SERIAL}"

if [ "${1:-}" = nebula ]; then
    $ADB shell am force-stop $PKG
    $ADB shell pm enable com.xreal.evapro.nebula
    echo "Nebula is back. Unplug and replug the glasses for it to take them."
    exit 0
fi

$ADB shell pm path $PKG >/dev/null || { echo "install Spatiand first: android/build.sh install"; exit 1; }
$ADB shell pm disable-user --user 0 com.xreal.evapro.nebula
$ADB shell appops set $PKG SYSTEM_ALERT_WINDOW allow
echo "Spatiand may draw over other apps"
UID_=$($ADB shell pm list packages -U $PKG | sed -n "s/^package:$PKG uid://p" | tr -d '\r' | cut -d, -f1)
APK=$($ADB shell pm path $PKG | head -1 | cut -d: -f2 | tr -d '\r')
$ADB shell CLASSPATH="$APK" app_process / $PKG.Setup usb "$UID_"
echo "Done. From now on, plugging the glasses in opens Spatiand."
