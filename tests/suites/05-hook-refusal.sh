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
export LATCHBOARD_CONFIG="$SCRATCH/config"
export LATCHD_BIN="$LATCHD"
export LATCH_NOTE_NO_NOTIFY=1
export LATCH_NOTE_NO_PROCS=1

mkdir -p "$LATCHD_DIR" "$LATCH_HANDOVER_DIR" "$LATCHD_CONFIG"
cat << 'INNER_EOF' > "$LATCHD_CONFIG/nodes"
[storage]
ab1001
testhost-a
sources = feed.json
regions = lan-east
INNER_EOF

cat << 'INNER_EOF' > "$LATCHD_CONFIG/feed.json"
[{"title": "Disk shelf firmware advisory", "regions": ["lan-east"]}]
INNER_EOF

"$LATCH_NOTE" nodes check > /dev/null
"$LATCH_NOTE" notices fetch > /dev/null
grep -q "ab1001" "$LATCH_HANDOVER_DIR/notices.json"

"$LATCH_NOTE" hook pre-command --session loop-a --command "ssh ab1001 uptime"
"$LATCHD" status | grep -q "ab1001 held by loop-a"

if "$LATCH_NOTE" hook pre-command --session loop-b --command "grep error /var/log/syslog | grep ab1001" 2>/dev/null; then
    exit 1
fi

"$LATCH_NOTE" hook pre-command --session loop-b --command "latch-note send loop-a ab1001 --note ping" > /dev/null

printf '{"prompt": "investigate ab1001 immediately", "source": "user"}' | "$LATCH_NOTE" hook prompt --session incident
"$LATCHD" take ab1001 --session incident | grep -q "incident take: ab1001 claimed by incident at p9"
