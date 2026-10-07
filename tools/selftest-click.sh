#!/usr/bin/env bash
# Clicks in a Chromium window the way the room clicks in a window of this Mac, and reports what the page saw.
# It takes the pointer and the front application for a few seconds, so it waits for the Mac to have been idle (FORCE=1 skips
# that). The page is Chrome's own app window, in a profile made here and thrown away.
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
binary="${1:-$here/mac/.build/debug/Spatiand}"
variant="${2:-all}"
idle() { ioreg -c IOHIDSystem | awk '/HIDIdleTime/ {print int($NF/1000000000); exit}'; }
if [ -z "${FORCE:-}" ]; then
  echo "waiting for the Mac to be idle for 60 s (FORCE=1 to skip)..."
  until [ "$(idle)" -ge 60 ]; do sleep 5; done
fi
work="$(mktemp -d /tmp/spatiand-click.XXXXXX)"
python3 "$here/tools/clicktest/server.py" "$work/log.txt" "$work/layout.json" & server=$!
trap 'kill $server 2>/dev/null; pkill -f "user-data-dir=$work/chrome" 2>/dev/null; rm -rf "$work"' EXIT
sleep 1
open -na "Google Chrome" --args --user-data-dir="$work/chrome" --no-first-run --no-default-browser-check --window-size=700,560 --window-position=300,120 --app=http://127.0.0.1:8765/
for i in $(seq 1 30); do [ -s "$work/layout.json" ] && break; sleep 1; done
CLICKTEST_LOG="$work/log.txt" CLICKTEST_LAYOUT="$work/layout.json" CLICKTEST_VARIANT="$variant" "$binary" --selftest-click
