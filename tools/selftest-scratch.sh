#!/usr/bin/env bash
# Run one of the Mac app's self-tests (`tools/selftest-scratch.sh --selftest-environment`) in a scratch home, with the data
# folders pointed into it, so that nothing of the wearer's own -- their environment, their controller layouts -- is touched.
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
flag="${1:?usage: selftest-scratch.sh --selftest-<name> [binary]}"
scratch="$(mktemp -d /tmp/spatiand-selftest.XXXXXX)"
trap 'rm -rf "$scratch"' EXIT
mkdir -p "$scratch/home" "$scratch/data/spatiand/environments" "$scratch/layouts"
binary="${2:-$here/mac/.build/debug/Spatiand}"
HOME="$scratch/home" XDG_DATA_HOME="$scratch/data" SPATIAND_ENVIRONMENTS="$scratch/data/spatiand/environments" SPATIAND_LAYOUTS="$scratch/layouts" \
    "$binary" "$flag"
