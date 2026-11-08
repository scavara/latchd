#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$1"
LATCH_RUN="$ROOT_DIR/target/debug/latch-run"
LATCH_UP="$ROOT_DIR/target/debug/latch-up"

SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT

export LATCHD_DIR="$SCRATCH/latchd"
export LATCH_HANDOVER_DIR="$SCRATCH/handover"
export LATCHBOARD_STATE_DIR="$SCRATCH/state"
export LATCHBOARD_CONFIG="$SCRATCH/config"
export LATCH_RUN_BIN="$LATCH_RUN"
export LATCH_NOTE_NO_NOTIFY=1
export LATCH_NOTE_NO_PROCS=1

mkdir -p "$LATCHD_DIR" "$LATCH_HANDOVER_DIR" "$LATCHBOARD_STATE_DIR" "$LATCHBOARD_CONFIG/loops"

MOCK_AGENT="$SCRATCH/mock-agent.sh"
cat << 'EOF' > "$MOCK_AGENT"
#!/usr/bin/env bash
while IFS= read -r line; do
    if [[ "$line" == *"TRIGGER_NEEDS_YOU"* ]]; then
        echo "[needs-you] waiting for user approval"
    fi
    echo "ack: $line" >> "${LATCHBOARD_STATE_DIR}/agent-seen.log"
done
EOF
chmod +x "$MOCK_AGENT"
export LATCH_AGENT_BIN="$MOCK_AGENT"

cat << 'EOF' > "$LATCHBOARD_CONFIG/common.md"
# initial rules
EOF

cat << 'EOF' > "$LATCHBOARD_CONFIG/loops/10-scrub.md"
title: Scrub Loop
every: 10m
restart_on: crash, upgrade, change
---
INITIAL_PROMPT TRIGGER_NEEDS_YOU
EOF

"$LATCH_UP" start scrub | grep -q "started scrub"

for _ in $(seq 1 20); do
    [ -S "$LATCHBOARD_STATE_DIR/sockets/scrub.sock" ] && break
    sleep 0.1
done

"$LATCH_UP" status | grep -q "scrub \[needs-you\]"
"$LATCH_UP" pause scrub | grep -q "paused"
"$LATCH_UP" status | grep -q "scrub \[paused\]"
"$LATCH_UP" resume scrub | grep -q "resumed"
"$LATCH_UP" tick scrub | grep -q "tick scheduled"

sleep 0.3
grep -q "TICK " "$LATCHBOARD_STATE_DIR/agent-seen.log"

echo "# updated rules" >> "$LATCHBOARD_CONFIG/common.md"
sleep 0.4

ls "$LATCH_HANDOVER_DIR/handover/scrub/"*runner-handover.json >/dev/null
"$LATCH_UP" stop scrub | grep -q "stopped scrub"
