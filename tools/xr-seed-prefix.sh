#!/usr/bin/env bash
# Let a Proton game's OpenXR reach Spatiand's runtime.
#
# Proton runs a Windows game's OpenXR through `wineopenxr`, which refuses to start unless the
# prefix's registry says `HKCU\Software\Wine\VR` has `state = 1` and names the Vulkan extensions the
# runtime wants. Proton fills those in only when SteamVR's OpenVR runtime is installed
# (`PROTON_VR_RUNTIME`); without it `state` is -1, wineopenxr answers XR_ERROR_INITIALIZATION_FAILED
# before it ever asks a runtime anything, and the game reports "no OpenXR runtime". The Deck has no
# SteamVR, and Spatiand's runtime is what should answer.
#
# This writes those values itself, once per prefix, into the prefix's `user.reg`. The key is
# non-volatile on purpose: Proton creates it volatile and, finding it already there, leaves it alone.
#
#   tools/xr-seed-prefix.sh 1763510          # Steam app id of the game
#
# Run it with the game closed. The extension lists must match `VULKAN_INSTANCE_EXTENSIONS` and
# `VULKAN_DEVICE_EXTENSIONS` in crates/spatiand-openxr/src/api.rs; the GPU's PCI ids are read from the
# machine. It keeps a copy of the file as `user.reg.bak-before-xr`.
set -euo pipefail

APPID="${1:?usage: xr-seed-prefix.sh <steam app id>}"
STEAM="${STEAM_ROOT:-$HOME/.local/share/Steam}"
PROTON="${PROTON_DIR:-$STEAM/steamapps/common/Proton 11.0}"
PREFIX="$STEAM/steamapps/compatdata/$APPID/pfx"
REG="$PREFIX/user.reg"

[[ -f "$REG" ]] || { echo "no prefix for $APPID yet: start the game once so Proton makes it" >&2; exit 1; }

# A wineserver still running would write its own copy of the registry over ours when it exits.
WINEPREFIX="$PREFIX" "$PROTON/files/bin/wineserver" -k 2>/dev/null || true
sleep 2

VID=$(printf '%08x' "$(cat /sys/class/drm/card0/device/vendor)")
PID=$(printf '%08x' "$(cat /sys/class/drm/card0/device/device)")

cp -n "$REG" "$REG.bak-before-xr"
python3 - "$REG" "$VID" "$PID" <<'PY'
import sys
path, vid, pid = sys.argv[1:4]
text = open(path, encoding="utf-8", errors="surrogateescape").read()
section = "[Software\\\\Wine\\\\VR]"
instance = "VK_KHR_get_physical_device_properties2 VK_KHR_external_memory_capabilities VK_KHR_external_semaphore_capabilities"
device = "VK_KHR_external_memory VK_KHR_external_memory_fd VK_EXT_external_memory_dma_buf VK_KHR_dedicated_allocation VK_KHR_get_memory_requirements2"
block = (
    f'{section} 1790000000\n#time=1dc0000000000000\n'
    f'"openxr_vulkan_device_extensions"="{device}"\n'
    f'"openxr_vulkan_device_pid"=dword:{pid}\n'
    f'"openxr_vulkan_device_vid"=dword:{vid}\n'
    f'"openxr_vulkan_instance_extensions"="{instance}"\n'
    f'"state"=dword:00000001\n'
)
if section in text:
    # Replace the existing section: from its header to the next blank line.
    start = text.index(section)
    end = text.find("\n\n", start)
    end = len(text) if end < 0 else end + 1
    text = text[:start] + block + text[end:]
    print("replaced the existing VR section")
else:
    text = text.rstrip("\n") + "\n\n" + block
    print("added the VR section")
open(path, "w", encoding="utf-8", errors="surrogateescape").write(text)
PY
echo "seeded HKCU\\Software\\Wine\\VR in $PREFIX (gpu $VID:$PID)"
