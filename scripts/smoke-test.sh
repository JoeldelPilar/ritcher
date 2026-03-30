#!/usr/bin/env bash
set -euo pipefail

BASE="${RITCHER_URL:-http://localhost:3000}"
PASS=0
FAIL=0

check() {
  local name="$1" url="$2" expect="$3"
  local body
  if body=$(curl -sf --max-time 5 "$url") && echo "$body" | grep -q "$expect"; then
    echo "  PASS: $name"
    ((PASS++))
  else
    echo "  FAIL: $name"
    ((FAIL++))
  fi
}

echo "Ritcher Smoke Test"
echo "=================="
echo "Target: $BASE"
echo ""

check "Health check" "$BASE/health" '"status"'
check "Dev UI" "$BASE/dev" "Ritcher Dev"
check "Demo HLS playlist" "$BASE/demo/playlist.m3u8" "#EXTM3U"
check "Demo DASH manifest" "$BASE/demo/manifest.mpd" "MPD"

# Stitched endpoints (uses demo as origin via config default)
SESSION="smoke-$(date +%s)"
check "Stitched HLS" "$BASE/stitch/$SESSION/playlist.m3u8" "#EXTM3U"
check "Stitched DASH" "$BASE/stitch/$SESSION/manifest.mpd" "MPD"

echo ""
echo "Results: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ] || exit 1
