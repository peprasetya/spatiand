#!/bin/sh
# SpatiMesh's Chromium, opened as a Wayland window of the room, with a profile of its own.
here=$(dirname "$(readlink -f "$0")")
export CHROME_DESKTOP=spatiand-chromium.desktop
# glibc 2.35+ sorts libraries by a newer method that sometimes trips its own assertion
# (dl-sort-maps.c, _dl_sort_maps_dfs) as Chromium loads its toolkit; the older method does not.
export GLIBC_TUNABLES=glibc.rtld.dynamic_sort=1
exec "$here/chrome" --ozone-platform=wayland --user-data-dir="$HOME/.config/spatiand-chromium" "$@"
