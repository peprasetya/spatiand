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
# 4. "All files access", for Spatiand's own applications to see shared storage.
# 5. Android's monitor of "phantom" processes off: Spatiand's applications are programs the
#    session starts (each SpatiQuill and SpatiShelf tab one more), which Android 14 otherwise
#    kills beyond 32 of them across all apps.
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
$ADB shell appops set --uid $PKG MANAGE_EXTERNAL_STORAGE allow
echo "Spatiand's applications may see shared storage"
$ADB shell settings put global settings_enable_monitor_phantom_procs false
$ADB shell device_config set_sync_disabled_for_tests persistent
$ADB shell device_config put activity_manager max_phantom_processes 2147483647
echo "Android leaves Spatiand's applications' processes alone"
echo "Done. From now on, plugging the glasses in opens Spatiand."
