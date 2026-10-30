#!/bin/sh
# SpatiMesh's Chromium, opened as a Wayland window of the room, with a profile of its own.
#
# Every feature switch is here: Chromium honours only the last --enable-features it is given,
# so the desktop entry names --ozone-platform itself, which keeps the room from adding switches
# of its own after these (spatiand-platform's wayland_arguments).
here=$(dirname "$(readlink -f "$0")")
export CHROME_DESKTOP=spatiand-chromium.desktop
# glibc 2.35+ sorts libraries by a newer method that sometimes trips its own assertion
# (dl-sort-maps.c, _dl_sort_maps_dfs) as Chromium loads its toolkit; the older method does not.
export GLIBC_TUNABLES=glibc.rtld.dynamic_sort=1
# Video decoded by the GPU (VA-API) -- VP9, H.264 -- as software cannot keep up with what
# YouTube sends a large window; VaapiIgnoreDriverChecks lets VA-API run on AMD's Mesa driver.
# HEVC never goes to VA-API (patch 0003: it has reset this GPU family); Spatiand's FFmpeg decodes
# it in software.
# No back and forward by a sideways swipe: on the Deck's pads that is a thumb's drift, and a
# swipe taken for it scrolls nothing.
exec "$here/chrome" --ozone-platform=wayland --user-data-dir="$HOME/.config/spatiand-chromium" \
    --password-store=basic --no-first-run --no-default-browser-check \
    --overscroll-history-navigation=0 \
    --enable-features=AcceleratedVideoDecodeLinuxGL,AcceleratedVideoDecodeLinuxZeroCopyGL,VaapiIgnoreDriverChecks,WaylandWindowDecorations \
    "$@"
