#!/usr/bin/env bash
set -euo pipefail

SESSION="latchd-ticks"
PROJECT_DIR="$HOME/projects/latchd"
STATE_DIR="${LATCHBOARD_STATE_DIR:-$HOME/.local/state/latchboard}"

clean_runners() {
    pkill -TERM -f "latch-run" 2>/dev/null || true
    rm -f "$STATE_DIR"/runners/*.json "$STATE_DIR"/sockets/*.sock 2>/dev/null || true
}

case "${1:-}" in
    --stop|stop)
        clean_runners
        if tmux has-session -t "$SESSION" 2>/dev/null; then
            tmux kill-session -t "$SESSION" 2>/dev/null || true
        fi
        printf "Stopped '%s'.\n" "$SESSION"
        exit 0
        ;;
    --restart|restart)
        if [ -n "${TMUX:-}" ]; then
            nohup bash -c "sleep 0.2; '$0' --stop >/dev/null 2>&1; '$0' --detached" >/dev/null 2>&1 &
            printf "Restarting '%s' (re-run start-ticks.sh to attach)...\n" "$SESSION"
            exit 0
        else
            clean_runners
            tmux kill-session -t "$SESSION" 2>/dev/null || true
            sleep 0.3
        fi
        ;;
    --status|status)
        if tmux has-session -t "$SESSION" 2>/dev/null; then
            tmux list-windows -t "$SESSION"
        else
            printf "Session '%s' is not running.\n" "$SESSION"
        fi
        exit 0
        ;;
esac

if tmux has-session -t "$SESSION" 2>/dev/null; then
    if [ -n "${TMUX:-}" ]; then
        tmux switch-client -t "$SESSION"
    else
        tmux attach-session -t "$SESSION"
    fi
    exit 0
fi

cd "$PROJECT_DIR"

TICKS=(
    "10-storage-scrub"
    "30-cleanup"
    "auth-renew"
    "cert-rotate"
    "telemetry"
    "99-incident"
)

clean_runners

tmux new-session -d -s "$SESSION" -c "$PROJECT_DIR" -n "${TICKS[0]}"

tmux set-option -t "$SESSION" status-position top
tmux set-option -t "$SESSION" mouse off
tmux set-option -t "$SESSION" status-left " [latchd] "
tmux set-option -t "$SESSION" status-left-length 12
tmux set-option -t "$SESSION" status-right " %H:%M %d-%b-%y "
tmux set-option -t "$SESSION" status-style "bg=#1e1e2e,fg=#cdd6f4"
tmux set-window-option -t "$SESSION" window-status-format " #[bg=#313244,fg=#cdd6f4] #I:#W "
tmux set-window-option -t "$SESSION" window-status-current-format " #[bg=#a6e3a1,fg=#11111b,bold] #I:#W* "

tmux send-keys -t "$SESSION":1 "latch-run examples/ticks/${TICKS[0]}.md" C-m

idx=2
for t in "${TICKS[@]:1}"; do
    tmux new-window -t "$SESSION":$idx -c "$PROJECT_DIR" -n "$t"
    tmux send-keys -t "$SESSION":$idx "latch-run examples/ticks/$t.md" C-m
    idx=$((idx + 1))
done

tmux new-window -t "$SESSION":$idx -c "$PROJECT_DIR" -n "dashboard"
tmux send-keys -t "$SESSION":$idx "latchboard" C-m

tmux select-window -t "$SESSION":$idx

if [[ "${1:-}" == "--detached" ]]; then
    exit 0
fi

if [ -n "${TMUX:-}" ]; then
    tmux switch-client -t "$SESSION"
else
    tmux attach-session -t "$SESSION"
fi
