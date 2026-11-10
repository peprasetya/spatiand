#!/bin/sh
# live.sh <url>...: SpatiMesh into the running Spatiand.app (its own runtime folder)
RT=/var/folders/dk/ztc9bs2j3378b2qnlb5bq8lw0000gn/T/spatiand-runtime
SRC=$HOME/Developer/spatiand-chromium-mac/chromium/src
SPATIAND_APPS_CONFIG=/tmp/smtest/apps exec env WAYLAND_DISPLAY=wayland-1 XDG_RUNTIME_DIR=$RT "$SRC/out/mac-ozone/SpatiMesh.app/Contents/MacOS/SpatiMesh" --ozone-platform=wayland --disable-gpu --user-data-dir=/tmp/smtest/profile-live --no-first-run --no-default-browser-check --class=spatimesh-chromium --autoplay-policy=no-user-gesture-required --enable-logging=stderr "$@"
