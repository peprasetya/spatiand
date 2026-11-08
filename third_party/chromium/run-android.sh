#!/system/bin/sh
# SpatiMesh's Chromium on the Beam Pro, as run.sh is on the Deck: a Wayland window of the room, with
# a profile of its own. Unpacked by the app with Chromium's resources (android/build.sh) and started
# by the room's launcher through sh (crates/spatiand/src/own.rs), as the app's files may not be run
# themselves; the program is in the APK's library folder, which the room names in SPATIAND_APPS_DIR.
#
# The switches are the Deck's (run.sh) but for VA-API, which Android has not got, and the GPU:
# Chromium draws in software here and hands the room its pictures as shared memory, until it draws
# with Android's GPU. Its sandbox is Chromium's second layer, the seccomp filter on every process
# (android-libc/0013): Android lets no app make namespaces, and there is no setuid helper
# (--disable-setuid-sandbox), so the first layer cannot be had here.
here=${0%/*}
# Its resources, which a Linux build finds beside the program (android-libc/0010).
export SPATIMESH_ASSETS="$here"
# Android's fonts, through Chromium's own fontconfig, its cache in the app's.
mkdir -p "$XDG_CACHE_HOME/fontconfig"
sed "s|CACHE|$XDG_CACHE_HOME/fontconfig|" "$here/fonts.conf" > "$XDG_CACHE_HOME/spatimesh-fonts.conf"
export FONTCONFIG_FILE="$XDG_CACHE_HOME/spatimesh-fonts.conf"
exec "$SPATIAND_APPS_DIR/libspatimesh_chromium.so" --ozone-platform=wayland --user-data-dir="$HOME/.config/spatiand-chromium" \
    --class=spatimesh-chromium \
    --password-store=basic --no-first-run --no-default-browser-check \
    --overscroll-history-navigation=0 --disable-gpu --disable-setuid-sandbox \
    --enable-features=EnableExtensionAutoupdate,ProfilesReordering,WaylandWindowDecorations \
    --disable-features=SigninPromoOnAvatarPill \
    "$@"
