#!/bin/sh
# What the room starts: SpatiMesh with the room's own switches, on a test profile and a test
# copy of Spatiand's shared folder (never the owner's: SPATIAND_APPS_CONFIG).
SPATIAND_APPS_CONFIG=/tmp/smtest/apps exec "$HOME/Developer/spatiand-chromium-mac/chromium/src/out/mac-ozone/SpatiMesh.app/Contents/MacOS/SpatiMesh" --user-data-dir=/tmp/smtest/profile --class=spatimesh-chromium --no-first-run --no-default-browser-check --overscroll-history-navigation=0 --enable-features=EnableExtensionAutoupdate,ProfilesReordering,WaylandWindowDecorations --disable-features=SigninPromoOnAvatarPill --ozone-platform=wayland --enable-logging=stderr $(cat /tmp/smtest/flags) > /tmp/smtest/sm.log 2>&1
