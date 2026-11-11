#!/bin/sh
# What the room starts: SpatiMesh with the room's own switches, on a test profile and a test
# copy of Spatiand's shared folder (never the owner's: SPATIAND_APPS_CONFIG).
# SM_NOFEAT: more features to turn off. SM_APP: another build of it (a copy kept from before a rebuild, or the one made ready to install).
APP=${SM_APP:-$HOME/Developer/spatiand-chromium-mac/chromium/src/out/mac-ozone/SpatiMesh.app}
SPATIAND_APPS_CONFIG=/tmp/smtest/apps exec "$APP/Contents/MacOS/SpatiMesh" --user-data-dir=/tmp/smtest/profile --class=spatimesh-chromium --no-first-run --no-default-browser-check --overscroll-history-navigation=0 --enable-features=EnableExtensionAutoupdate,ProfilesReordering,WaylandWindowDecorations --disable-features=SigninPromoOnAvatarPill${SM_NOFEAT:+,$SM_NOFEAT} --ozone-platform=wayland --enable-logging=stderr $(cat /tmp/smtest/flags) > /tmp/smtest/sm.log 2>&1
