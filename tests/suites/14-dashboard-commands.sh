#!/usr/bin/env bash
set -euo pipefail

# Drives the interactive dashboard inside a private tmux server.
if ! command -v tmux >/dev/null 2>&1; then
    echo "SKIP 14-dashboard-commands.sh: tmux not installed"
    exit 0
fi

ROOT_DIR="$1"
DASH_DIR="$ROOT_DIR/dashboard"
LATCHD="$ROOT_DIR/target/debug/latchd"
LATCH_RUN="$ROOT_DIR/target/debug/latch-run"
LATCH_UP="$ROOT_DIR/target/debug/latch-up"
LATCH_NOTE="$ROOT_DIR/bin/latch-note"

SCRATCH="$(mktemp -d)"
TMUX_SOCK="latchd-test-$$"
tm() { tmux -L "$TMUX_SOCK" "$@"; }
trap 'tm kill-server >/dev/null 2>&1 || true; "$LATCH_UP" stop all >/dev/null 2>&1 || true; rm -rf "$SCRATCH"' EXIT

export LATCHD_DIR="$SCRATCH/latchd"
export LATCH_HANDOVER_DIR="$SCRATCH/handover"
export LATCHBOARD_STATE_DIR="$SCRATCH/state"
export LATCHBOARD_CONFIG="$SCRATCH/config"
export LATCHD_CONFIG="$SCRATCH/config"
export LATCH_CONFIG_FILE="$SCRATCH/config/config"
export LATCH_RUN_BIN="$LATCH_RUN"
export LATCH_UP_BIN="$LATCH_UP"
export LATCH_NOTE_BIN="$LATCH_NOTE"
export LATCHD_BIN="$LATCHD"
export LATCH_NOTE_NO_NOTIFY=1
export LATCH_PROMPT_WAIT_MS=200

mkdir -p "$LATCHD_DIR" "$LATCH_HANDOVER_DIR" "$LATCHBOARD_STATE_DIR" "$LATCHBOARD_CONFIG/ticks"
printf "ab1001\nab1002\n" > "$LATCHD_CONFIG/nodes"

MOCK_AGENT="$SCRATCH/mock-agent.sh"
cat << 'EOF' > "$MOCK_AGENT"
#!/usr/bin/env bash
while IFS= read -r line; do :; done
EOF
chmod +x "$MOCK_AGENT"
export LATCH_AGENT_BIN="$MOCK_AGENT"

for t in 10-alpha 20-beta; do
    cat << EOF > "$LATCHBOARD_CONFIG/ticks/$t.md"
title: Tick $t
every: 10m
restart_on: upgrade
---
hello
EOF
done

# state for every panel
"$LATCHD" claim ab1001 --holder alpha -p 7 > /dev/null
"$LATCH_NOTE" maintenance "PSU swap" ab1002 --from +2h --until +3h > /dev/null
"$LATCH_NOTE" directive "Flaky NIC" ab1001 --until +1h --warn > /dev/null
"$LATCH_NOTE" rollout start bios-update ab1001 > /dev/null
"$LATCH_UP" start all > /dev/null
for _ in $(seq 1 30); do
    [ -f "$LATCHBOARD_STATE_DIR/runners/alpha.json" ] && [ -f "$LATCHBOARD_STATE_DIR/runners/beta.json" ] && break
    sleep 0.1
done

cargo build --manifest-path="$DASH_DIR/Cargo.toml" --quiet
DASH_BIN="$DASH_DIR/target/debug/latchboard-dashboard"

# the text snapshot lists directives and maintenance too
ONCE="$("$DASH_BIN" --once)"
echo "$ONCE" | grep -q "Directives (1):"
echo "$ONCE" | grep -q "#2 warn on ab1001 (ends in .*): Flaky NIC"
echo "$ONCE" | grep -q "#1 block on ab1002 (starts in .*): PSU swap"

tm -f /dev/null new-session -d -s dash -x 160 -y 45 "$DASH_BIN"

screen() { tm capture-pane -t dash -p; }
# wait until the screen shows (or stops showing) a pattern
expect() {
    for _ in $(seq 1 50); do
        if screen | grep -q -- "$1"; then return 0; fi
        sleep 0.1
    done
    echo "expected on screen: $1" >&2
    screen >&2
    return 1
}
expect_gone() {
    for _ in $(seq 1 50); do
        if ! screen | grep -q -- "$1"; then return 0; fi
        sleep 0.1
    done
    echo "still on screen: $1" >&2
    return 1
}
keys() { tm send-keys -t dash "$@"; }

# the grid shows every panel with its key
expect "Overview \[o\]"
expect "Runners (2) \[w\]"
expect "Maintenance (1) \[m\]"
expect "Directives (1) \[d\]"
expect "Host queues \[h\]"
expect "Notices (0) \[n\]"
expect " 1  alpha .* running"
expect " 2  beta .* running"

# a panel key shows that panel alone; Esc returns to the grid
keys m
expect "#1 block on ab1002 (starts in .*): PSU swap"
expect_gone "Runners (2)"
expect "Esc back"
keys Escape
expect "Runners (2) \[w\]"
keys w
expect_gone "Maintenance (1)"
keys w
expect "Maintenance (1) \[m\]"

# an unknown command and a bad target are reported, nothing runs
keys "/reboot" Enter
expect "unknown command /reboot"
keys "/shutdown 9" Enter
expect "no runner #9 (2 running)"

# /shutdown N asks first; anything but y cancels
keys "/shutdown 1" Enter
expect "Stop tick 'alpha'? (y/n)"
keys n
expect "cancelled"
[ -f "$LATCHBOARD_STATE_DIR/runners/alpha.json" ]

# confirmed, it stops that tick through latch-up and leaves the other running
ALPHA_PID="$(python3 -c 'import json, sys; print(json.load(open(sys.argv[1]))["pid"])' "$LATCHBOARD_STATE_DIR/runners/alpha.json")"
keys "/shutdown 1" Enter
expect "Stop tick 'alpha'? (y/n)"
keys y
expect "stopped alpha"
expect "Runners (1) \[w\]"
expect " 1  beta .* running"
for _ in $(seq 1 40); do
    kill -0 "$ALPHA_PID" 2>/dev/null || break
    sleep 0.1
done
! kill -0 "$ALPHA_PID" 2>/dev/null
"$LATCH_UP" status | grep -q "beta \[running\]"

# /close marks a rollout done through latch-note
keys "/close bios-update" Enter
expect "Mark rollout 'bios-update' as done? (y/n)"
keys y
expect "rollout #1 state -> done"
"$LATCH_NOTE" rollout show 1 --json | grep -q '"state": "done"'
keys "/close nope" Enter
keys y
expect "error: .*rollout not found: nope"

# /shutdown all stops the rest; q leaves the dashboard
keys "/shutdown all" Enter
expect "Stop ALL ticks? (y/n)"
keys y
expect "stopped beta"
expect "Runners (0) \[w\]"
keys q
for _ in $(seq 1 30); do
    tm has-session -t dash 2>/dev/null || break
    sleep 0.1
done
! tm has-session -t dash 2>/dev/null
