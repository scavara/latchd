#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$1"
LATCHD="$ROOT_DIR/target/debug/latchd"

SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT

export LATCHD_DIR="$SCRATCH/latchd"
export LATCH_HANDOVER_DIR="$SCRATCH/handover"
export LATCHD_CONFIG="$SCRATCH/config"
export LATCH_NOTE_NO_NOTIFY=1
export LATCH_NOTE_NO_PROCS=1

mkdir -p "$LATCHD_DIR" "$LATCH_HANDOVER_DIR" "$LATCHD_CONFIG"
printf "ab1001\ntesthost-a\n" > "$LATCHD_CONFIG/nodes"

"$LATCHD" claim ab1001 --holder tick-a -p 5 | grep -q "claimed ab1001"

if "$LATCHD" claim ab1001 --holder tick-b -p 6 2>/dev/null; then
    exit 1
fi

"$LATCHD" wait ab1001 --waiter tick-b -p 5 | grep -q "queued tick-b"

sqlite3 "$LATCHD_DIR/claims.db" "UPDATE queue SET enqueued_at = enqueued_at - 3700 WHERE node = 'ab1001';"
"$LATCHD" sweep | grep -q "yield requested on ab1001: tick-b (p7) vs tick-a (p5)"

"$LATCHD" grant-take ab1001 --session incident
"$LATCHD" take ab1001 --session incident | grep -q "incident take: ab1001 claimed by incident at p9"

"$LATCHD" claim unknown-99 --holder tick-a -p 4 | grep -q "claimed unknown-99"
sqlite3 "$LATCHD_DIR/claims.db" "UPDATE claims SET last_used_at = last_used_at - 700 WHERE node = 'unknown-99';"
"$LATCHD" sweep | grep -q "released unknown idle node unknown-99"
