#!/bin/sh
exec "$HOME/Developer/spatiand-chromium-mac/chromium/src/out/mac-ozone/SpatiMesh.app/Contents/MacOS/SpatiMesh" --ozone-platform=wayland --disable-gpu --user-data-dir=/tmp/smtest/profile --no-first-run --no-default-browser-check --class=spatimesh-chromium --enable-logging=stderr $(cat /tmp/smtest/flags) > /tmp/smtest/sm.log 2>&1
