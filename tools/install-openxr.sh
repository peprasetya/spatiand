#!/usr/bin/env bash
# Put Spatiand's OpenXR runtime where the OpenXR loader (and so a game) can find it.
#
#   tools/install-openxr.sh            install it; applications opt in with XR_RUNTIME_JSON
#   tools/install-openxr.sh --default  ...and make it this user's active OpenXR runtime
#
# Installs `libspatiand_openxr.so` and a manifest under ~/.local/share/spatiand-openxr/. The library is
# copied, so rebuilding does not change what a running game has loaded. Nothing outside the home
# folder is touched.
#
# The session's own environment gets `XR_RUNTIME_JSON` too (~/.config/spatiand/session.env), so a
# game started from inside Spatiand finds the runtime without being told. `--default` is for
# running OpenXR programs from a terminal or a launcher that Spatiand did not start; it replaces
# whatever runtime (SteamVR, Monado) this user had as their default.
set -euo pipefail

REPO="${SPATIAND_REPO:-$(cd "$(dirname "$0")/.." && pwd)}"
LIB="${1:-}"
DEFAULT=0
[[ "${1:-}" == "--default" ]] && { DEFAULT=1; LIB=""; }
[[ "${2:-}" == "--default" ]] && DEFAULT=1
LIB="${LIB:-$REPO/target/release/libspatiand_openxr.so}"
[[ -f "$LIB" ]] || { echo "not built: $LIB" >&2; echo "cargo build --release -p spatiand-openxr" >&2; exit 1; }

DEST="$HOME/.local/share/spatiand-openxr"
mkdir -p "$DEST"
cp -f "$LIB" "$DEST/libspatiand_openxr.so"
cat > "$DEST/spatiand_openxr.json" <<JSON
{
    "file_format_version": "1.0.0",
    "runtime": {
        "name": "Spatiand",
        "library_path": "$DEST/libspatiand_openxr.so"
    }
}
JSON
echo "installed $DEST/spatiand_openxr.json"

# The session's environment.
ENV="$HOME/.config/spatiand/session.env"
mkdir -p "$(dirname "$ENV")"
touch "$ENV"
grep -q '^XR_RUNTIME_JSON=' "$ENV" || echo "XR_RUNTIME_JSON=$DEST/spatiand_openxr.json" >> "$ENV"
echo "session.env points XR_RUNTIME_JSON at it"

if [[ "$DEFAULT" == 1 ]]; then
    mkdir -p "$HOME/.config/openxr/1"
    ln -sf "$DEST/spatiand_openxr.json" "$HOME/.config/openxr/1/active_runtime.json"
    echo "made it this user's active OpenXR runtime"
fi
echo "log: ~/.local/share/spatiand-openxr.log"
