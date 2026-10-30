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
# ProfilesReordering: the profiles' order is the one every machine shares.
# EnableExtensionAutoupdate: extensions stay up to date, as in Chrome, and one installed on another
# machine can be fetched here (off by default in Cromite's patches).
# The program is named spatimesh, not chrome: SpatiKey names who is asking from what is running.
# --class is each window's Wayland app id, by which the room gives every SpatiMesh window a sink of
# its own for its tabs' sound (crates/spatiand/src/audio.rs, CHROMIUM_APP_ID).
exec "$here/spatimesh" --ozone-platform=wayland --user-data-dir="$HOME/.config/spatiand-chromium" \
    --class=spatimesh-chromium \
    --password-store=basic --no-first-run --no-default-browser-check \
    --overscroll-history-navigation=0 \
    --enable-features=AcceleratedVideoDecodeLinuxGL,AcceleratedVideoDecodeLinuxZeroCopyGL,EnableExtensionAutoupdate,ProfilesReordering,VaapiIgnoreDriverChecks,WaylandWindowDecorations \
    "$@"
