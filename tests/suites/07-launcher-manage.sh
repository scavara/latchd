#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$1"

SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT

export LATCH_HOME="$SCRATCH/home"
export LATCH_PREFIX_BIN="$LATCH_HOME/.local/bin"
export LATCHBOARD_STATE_DIR="$SCRATCH/state"
export CLAUDE_SETTINGS_PATH="$LATCH_HOME/.claude/settings.json"
export LATCH_SKIP_TESTS=1

mkdir -p "$LATCH_HOME" "$LATCH_PREFIX_BIN" "$LATCHBOARD_STATE_DIR"

"$ROOT_DIR/manage.sh" check | grep -q "manifest and hooks ok"
"$ROOT_DIR/manage.sh" status | grep -q "MISSING"
"$ROOT_DIR/manage.sh" install | grep -q "installed"
"$ROOT_DIR/manage.sh" status | grep -q "UPTODATE"
"$ROOT_DIR/manage.sh" hooks | grep -q "merged hooks"
grep -q "latch-note hook pre-command" "$CLAUDE_SETTINGS_PATH"

touch "$LATCH_PREFIX_BIN/latchd"
cp "$LATCH_PREFIX_BIN/latchd" "$LATCH_PREFIX_BIN/latchd.bak-20260101000001"
touch "$LATCH_PREFIX_BIN/latchd.bak-20260101000001.keep"
cp "$LATCH_PREFIX_BIN/latchd" "$LATCH_PREFIX_BIN/latchd.bak-20260101000002"
cp "$LATCH_PREFIX_BIN/latchd" "$LATCH_PREFIX_BIN/latchd.bak-20260101000003"

"$ROOT_DIR/install.sh" latchd | grep -q "installed latchd"

[ -f "$LATCH_PREFIX_BIN/latchd.bak-20260101000001" ]
[ ! -f "$LATCH_PREFIX_BIN/latchd.bak-20260101000002" ]
