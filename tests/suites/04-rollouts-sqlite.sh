#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$1"
LATCH_NOTE="$ROOT_DIR/bin/latch-note"

SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT

export LATCHD_DIR="$SCRATCH/latchd"
export LATCH_HANDOVER_DIR="$SCRATCH/handover"
export LATCHD_CONFIG="$SCRATCH/config"
export LATCH_NOTE_NO_NOTIFY=1
export LATCH_NOTE_NO_PROCS=1

mkdir -p "$LATCHD_DIR" "$LATCH_HANDOVER_DIR" "$LATCHD_CONFIG"

"$LATCH_NOTE" rollout start kernel-patch ab1001 testhost-a --owner loop-sec --phase canary | grep -q "rollout #1"
"$LATCH_NOTE" rollout mark kernel-patch ab1001 done --note "rebooted cleanly" | grep -q "ab1001 -> done"
"$LATCH_NOTE" rollout phase 1 batch-1 | grep -q "phase -> batch-1"
"$LATCH_NOTE" rollout note 1 "canary green" | grep -q "note updated"

SHOW_JSON="$("$LATCH_NOTE" rollout show 1 --json)"
echo "$SHOW_JSON" | grep -q '"phase": "batch-1"'
echo "$SHOW_JSON" | grep -q '"status": "done"'

"$LATCH_NOTE" rollout state 1 done | grep -q "state -> done"
"$LATCH_NOTE" rollout ls | grep -q "#1 kernel-patch \[done/batch-1\]"

sqlite3 "$LATCH_HANDOVER_DIR/rollouts.db" "UPDATE rollouts SET finished_at = finished_at - 90000 WHERE id = 1;"
if "$LATCH_NOTE" rollout ls | grep -q "kernel-patch"; then
    exit 1
fi
"$LATCH_NOTE" rollout ls --all | grep -q "kernel-patch"

"$LATCH_NOTE" send loop-net ab1001 --from loop-sec --note "check bonding" | grep -q "handed over ab1001 to loop-net"
"$LATCH_NOTE" inbox loop-net --pop | grep -q "check bonding"
"$LATCH_NOTE" ask "Approve BIOS flash on ab1001?" ab1001 | grep -q "question #1"
"$LATCH_NOTE" answered 1 "approved" | grep -q "question #1 marked answered"
