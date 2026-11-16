#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$1"
LATCH_RUN="$ROOT_DIR/target/debug/latch-run"
LATCH_UP="$ROOT_DIR/target/debug/latch-up"

SCRATCH="$(mktemp -d)"
trap '"$LATCH_UP" stop all >/dev/null 2>&1 || true; rm -rf "$SCRATCH"' EXIT

export LATCHD_DIR="$SCRATCH/latchd"
export LATCH_HANDOVER_DIR="$SCRATCH/handover"
export LATCHBOARD_STATE_DIR="$SCRATCH/state"
export LATCHBOARD_CONFIG="$SCRATCH/config"
export LATCH_CONFIG_FILE="$SCRATCH/config/config"
export LATCH_RUN_BIN="$LATCH_RUN"
export LATCH_PROMPT_WAIT_MS=200

RUNNERS="$LATCHBOARD_STATE_DIR/runners"
SOCKETS="$LATCHBOARD_STATE_DIR/sockets"
mkdir -p "$LATCHD_DIR" "$LATCH_HANDOVER_DIR" "$RUNNERS" "$SOCKETS" "$LATCHBOARD_CONFIG/ticks"

MOCK_AGENT="$SCRATCH/mock-agent.sh"
cat << 'EOF' > "$MOCK_AGENT"
#!/usr/bin/env bash
while IFS= read -r line; do :; done
EOF
chmod +x "$MOCK_AGENT"
export LATCH_AGENT_BIN="$MOCK_AGENT"

cat << 'EOF' > "$LATCHBOARD_CONFIG/ticks/10-live.md"
title: Live Tick
every: 10m
restart_on: upgrade
---
hello
EOF

"$LATCH_UP" sweep | grep -q "nothing to sweep"

"$LATCH_UP" start live | grep -q "started live"
for _ in $(seq 1 30); do
    [ -S "$SOCKETS/live.sock" ] && [ -f "$RUNNERS/live.json" ] && break
    sleep 0.1
done

# a runner that died without cleaning up: registry entry with a dead PID plus its socket
true &
DEAD_PID=$!
wait "$DEAD_PID"
cat << EOF > "$RUNNERS/ghost.json"
{"tick_id": "ghost", "title": "Ghost", "pid": $DEAD_PID, "role": null, "every_secs": 600,
 "paused": false, "needs_you": false, "last_tick_ts": 0, "next_tick_ts": null,
 "socket_path": "$SOCKETS/ghost.sock"}
EOF
python3 -c 'import socket, sys; s = socket.socket(socket.AF_UNIX); s.bind(sys.argv[1]); s.close()' "$SOCKETS/ghost.sock"

# a socket nobody listens on and nothing registered
python3 -c 'import socket, sys; s = socket.socket(socket.AF_UNIX); s.bind(sys.argv[1]); s.close()' "$SOCKETS/orphan.sock"

OUT="$("$LATCH_UP" sweep)"
echo "$OUT" | grep -q "swept stale runner state for ghost"
echo "$OUT" | grep -q "swept stale runner state for orphan"
if echo "$OUT" | grep -q "live"; then
    exit 1
fi

[ ! -e "$RUNNERS/ghost.json" ]
[ ! -e "$SOCKETS/ghost.sock" ]
[ ! -e "$SOCKETS/orphan.sock" ]

# the live runner is untouched and still answers
[ -S "$SOCKETS/live.sock" ]
[ -f "$RUNNERS/live.json" ]
"$LATCH_UP" status | grep -q "live \[running\]"
"$LATCH_UP" sweep | grep -q "nothing to sweep"
"$LATCH_UP" stop live | grep -q "stopped live"
