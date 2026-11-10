#!/bin/sh
# Saves the Linux-UI Mac port's work in progress from littleotter's Chromium tree into this
# repository, so that nothing lives only on that machine (it is shut down at night, and the
# port is done in steps across days; docs/spatimesh-mac-port.md).
#
#   third_party/chromium/mac/save-wip.sh        run on deepmagpie; commits nothing itself
#
# wip/linux-ui.patch      Chromium's own files changed and the new files of the port, as one
#                         diff against the patched tree (patches 0001-0022, mac/0001-0002)
# wip/libwayland.patch    third_party/wayland/src, which Chromium's DEPS leaves out of a Mac
#                         checkout and was copied from the Deck's tree: its macOS changes, as a
#                         diff against the Deck's copy
#
# Applied again with `git apply` in the Mac tree (and `patch -p1` in third_party/wayland/src).
set -eu
cd "$(dirname "$0")"
MAC=Developer/spatiand-chromium-mac/chromium/src
DECK=/media/shared/Linux/App/spatiand-chromium/chromium/src
mkdir -p wip

# The port's new files, which git does not know of yet, said by name: the tree has other
# untracked files (its FFmpeg) that are not the port's.
NEW="ui/events/platform/wayland/wayland_event_watcher_mac.h
ui/events/platform/wayland/wayland_event_watcher_mac.cc
ui/ozone/platform/wayland/common/wayland_shm_mapping.h
ui/ozone/platform/wayland/common/wayland_shm_mapping.cc
third_party/spatiand_mac
chrome/browser/shell_integration_linux_ui_mac.cc
media/audio/spatiand"
ssh littleotter "cd $MAC && git add -N $(echo $NEW | tr '\n' ' ') && git diff --binary && git reset -q -- $(echo $NEW | tr '\n' ' ')" > wip/linux-ui.patch.new
mv wip/linux-ui.patch.new wip/linux-ui.patch

# libwayland: each file of it that differs from the Deck's copy.
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/a" "$tmp/b"
for f in src/wayland-os.c src/connection.c src/wayland-client.c; do
    mkdir -p "$tmp/a/$(dirname $f)" "$tmp/b/$(dirname $f)"
    cp "$DECK/third_party/wayland/src/$f" "$tmp/a/$f"
    scp -q "littleotter:$MAC/third_party/wayland/src/$f" "$tmp/b/$f"
done
(cd "$tmp" && diff -ru a b || true) > wip/libwayland.patch
echo "saved: $(grep -c '^diff --git' wip/linux-ui.patch) files in wip/linux-ui.patch, $(grep -c '^diff -ru' wip/libwayland.patch) in wip/libwayland.patch"
