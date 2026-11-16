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

mkdir -p "$LATCHD_DIR" "$LATCH_HANDOVER_DIR" "$LATCHBOARD_STATE_DIR" "$LATCHBOARD_CONFIG/ticks" "$LATCHBOARD_CONFIG/mcp"

MOCK_AGENT="$SCRATCH/mock-agent.sh"
cat << 'EOF' > "$MOCK_AGENT"
#!/usr/bin/env bash
echo "$LATCH_SESSION args: $* | fullscreen=${CLAUDE_CODE_NO_FLICKER:-0}" >> "${LATCHBOARD_STATE_DIR}/agent-start.log"
while IFS= read -r line; do :; done
EOF
chmod +x "$MOCK_AGENT"
export LATCH_AGENT_BIN="$MOCK_AGENT"

echo '{"mcpServers": {}}' > "$LATCHBOARD_CONFIG/mcp/shared.json"

cat << 'EOF' > "$LATCH_CONFIG_FILE"
agent_model = fable
agent_effort = low
agent_fullscreen = true

# matched by tick id
[incident]
model = opus
effort = high
mcp_config = mcp/shared.json
fullscreen = false

# matched by file name
[20-scrub]
mcp_config = mcp/shared.json, mcp/missing.json
EOF

for t in 10-plain 20-scrub 99-incident 30-own; do
    cat << 'EOF' > "$LATCHBOARD_CONFIG/ticks/$t.md"
title: T
every: 10m
restart_on: change
---
hello
EOF
done
# a tick's own agent_args beat both the section and the globals
cat << 'EOF' >> "$LATCH_CONFIG_FILE"

[own]
model = opus
EOF
sed -i 's/^restart_on: change$/restart_on: change\nagent_args: --model sonnet/' "$LATCHBOARD_CONFIG/ticks/30-own.md"

"$LATCH_UP" start all > /dev/null

LOG="$LATCHBOARD_STATE_DIR/agent-start.log"
for _ in $(seq 1 50); do
    [ -f "$LOG" ] && [ "$(wc -l < "$LOG")" -ge 4 ] && break
    sleep 0.1
done

MCP="$LATCHBOARD_CONFIG/mcp/shared.json"
grep -qxF "plain args: --model fable --effort low | fullscreen=1" "$LOG"
grep -qxF "scrub args: --model fable --effort low --mcp-config $MCP | fullscreen=1" "$LOG"
grep -qxF "incident args: --model opus --effort high --mcp-config $MCP | fullscreen=0" "$LOG"
grep -qxF "own args: --model sonnet --effort low | fullscreen=1" "$LOG"

# the missing MCP file was reported, not passed on
grep -q "mcp_config not found, skipped: .*mcp/missing.json" "$LATCHBOARD_STATE_DIR/logs/scrub.log"

# editing a section restarts the tick with the new settings
sed -i 's/^effort = high$/effort = max/' "$LATCH_CONFIG_FILE"
for _ in $(seq 1 50); do
    grep -qF "incident args: --model opus --effort max" "$LOG" && break
    sleep 0.1
done
grep -qxF "incident args: --model opus --effort max --mcp-config $MCP | fullscreen=0" "$LOG"
