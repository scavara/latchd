#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$1"
LATCHD="$ROOT_DIR/target/debug/latchd"
LATCH_NOTE="$ROOT_DIR/bin/latch-note"

SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT

export LATCHD_DIR="$SCRATCH/latchd"
export LATCH_HANDOVER_DIR="$SCRATCH/handover"
export LATCHD_CONFIG="$SCRATCH/config"
export LATCH_NOTE_NO_NOTIFY=1
export LATCH_NOTE_NO_PROCS=1

mkdir -p "$LATCHD_DIR" "$LATCH_HANDOVER_DIR" "$LATCHD_CONFIG"
printf "ab1001\ntesthost-a\n" > "$LATCHD_CONFIG/nodes"

"$LATCHD" claim ab1001 --holder loop-a -p 5 | grep -q "claimed ab1001"
"$LATCHD" claim testhost-a --holder loop-a -p 5 | grep -q "claimed testhost-a"

"$LATCH_NOTE" heartbeat "checking raid array" ab1001 --loop loop-a --tick-secs 600 | grep -q "heartbeat recorded"

sqlite3 "$LATCHD_DIR/claims.db" "UPDATE claims SET last_used_at = last_used_at - 3000;"

SWEEP_OUT="$("$LATCHD" sweep)"
echo "$SWEEP_OUT" | grep -q "warned idle node testhost-a"
if echo "$SWEEP_OUT" | grep -q "ab1001"; then
    exit 1
fi

sqlite3 "$LATCHD_DIR/claims.db" "UPDATE claims SET warned_at = warned_at - 700 WHERE node = 'testhost-a';"
"$LATCHD" sweep | grep -q "released idle node testhost-a"
"$LATCHD" status | grep -q "ab1001 held by loop-a"
