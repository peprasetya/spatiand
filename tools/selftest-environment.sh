#!/usr/bin/env bash
# Run the Mac app's environment picker test in a scratch home, so that nothing of the wearer's own is touched.
# Pictures of what the glasses would show are written to /tmp/spatiand-env-*.png.
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
scratch="$(mktemp -d /tmp/spatiand-selftest.XXXXXX)"
trap 'rm -rf "$scratch"' EXIT
mkdir -p "$scratch/home" "$scratch/data/spatiand/environments"
binary="${1:-$here/mac/.build/debug/Spatiand}"
HOME="$scratch/home" XDG_DATA_HOME="$scratch/data" SPATIAND_ENVIRONMENTS="$scratch/data/spatiand/environments" \
    "$binary" --selftest-environment
