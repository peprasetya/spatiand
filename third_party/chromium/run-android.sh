#!/system/bin/sh
# SpatiMesh's Chromium on the Beam Pro, as run.sh is on the Deck: a Wayland window of the room, with
# a profile of its own. Unpacked by the app with Chromium's resources (android/build.sh) and started
# by the room's launcher through sh (crates/spatiand/src/own.rs), as the app's files may not be run
# themselves; the program is in the APK's library folder, which the room names in SPATIAND_APPS_DIR.
#
# The switches are the Deck's (run.sh) but for VA-API, which Android has not got, and the GPU's: it
# draws with the GPU through ANGLE on Android's Vulkan (--use-angle=vulkan; Adreno's driver on the
# Beam Pro), off screen, each frame handed to the room as shared memory, as Android's EGL and Vulkan
# make no Wayland surfaces (android-libc/0014). Its sandbox is Chromium's second layer, the seccomp
# filter on every process (android-libc/0013): Android lets no app make namespaces, and there is no
# setuid helper (--disable-setuid-sandbox), so the first layer cannot be had here.
here=${0%/*}
# Its resources, which a Linux build finds beside the program (android-libc/0010).
export SPATIMESH_ASSETS="$here"
# Android's fonts, through Chromium's own fontconfig, its cache in the app's.
mkdir -p "$XDG_CACHE_HOME/fontconfig"
sed "s|CACHE|$XDG_CACHE_HOME/fontconfig|" "$here/fonts.conf" > "$XDG_CACHE_HOME/spatimesh-fonts.conf"
export FONTCONFIG_FILE="$XDG_CACHE_HOME/spatimesh-fonts.conf"
# Downloads into Android's own Download folder, where the phone's other apps find them, when
# Spatiand may write there ("All files access"); otherwise into Downloads in its own home.
shared=/storage/emulated/0/Download
if [ -d "$shared" ] && [ -w "$shared" ]; then
    printf 'XDG_DOWNLOAD_DIR="%s"\n' "$shared" > "$XDG_CONFIG_HOME/user-dirs.dirs"
elif [ -f "$XDG_CONFIG_HOME/user-dirs.dirs" ] && grep -q "$shared" "$XDG_CONFIG_HOME/user-dirs.dirs"; then
    rm -f "$XDG_CONFIG_HOME/user-dirs.dirs"
fi
# --disable-partial-raster: a workaround, not the fix. Re-rastering only the changed part of a tile
# into the tile's existing buffer, as Chromium does by default, left those buffers' other parts as
# whatever the GPU had there on the Beam Pro: black blocks and coloured speckles in pages, the page
# itself drawn shifted, a dropdown's list drawn into the page -- in the page's own picture, so before
# any frame reached the room (owner's screenshot of YouTube, 2026-11-09; reproduced with a <select>
# and gone with this switch). It costs whole tiles re-rastered where part would do. The proper
# alternative is to find why a tile's buffer does not keep its content on this path (ANGLE on
# Adreno's Vulkan, Chromium's Linux shared images built for Android's libc) and fix that there.
exec "$SPATIAND_APPS_DIR/libspatimesh_chromium.so" --ozone-platform=wayland --user-data-dir="$HOME/.config/spatiand-chromium" \
    --class=spatimesh-chromium \
    --password-store=basic --no-first-run --no-default-browser-check \
    --overscroll-history-navigation=0 --use-angle=vulkan --disable-setuid-sandbox --disable-partial-raster \
    --enable-features=EnableExtensionAutoupdate,ProfilesReordering,WaylandWindowDecorations \
    --disable-features=SigninPromoOnAvatarPill \
    "$@"
