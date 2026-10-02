#!/bin/bash
# Screenshot the GUI in a given state (dev aid; builds with the dev-hooks feature).
# Usage: shot.sh <out.png> <WxH> <view> <frames> [image] [extra VAR=value ...]
set -u
OUT="${1:?}"; SIZE="$2"; VIEW="$3"; FRAMES="$4"; IMG="${5:-}"; shift 5 || shift $#
cd "$(dirname "$0")/.."
cargo build --release -q -p relume --features dev-hooks
rm -f -- "$OUT"
env RELUME_RENDERER=gl RELUME_SIZE="$SIZE" RELUME_SHOT_VIEW="$VIEW" RELUME_SCREENSHOT="$OUT" RELUME_SCREENSHOT_FRAMES="$FRAMES" "$@" ./target/release/Relume $IMG >/dev/null 2>&1 &
PID=$!
for i in $(seq 1 80); do sleep 0.5; [ -f "$OUT" ] && break; done
sleep 0.5; kill $PID 2>/dev/null
ls "$OUT"
