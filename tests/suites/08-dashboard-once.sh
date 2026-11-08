#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$1"
DASH_DIR="$ROOT_DIR/dashboard"

SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT

export LATCHD_DIR="$SCRATCH/latchd"
export LATCH_HANDOVER_DIR="$SCRATCH/handover"
export LATCHBOARD_STATE_DIR="$SCRATCH/state"
export LATCHBOARD_CONFIG="$SCRATCH/config"
export LATCHD_CONFIG="$SCRATCH/config"

mkdir -p "$LATCHD_DIR" "$LATCH_HANDOVER_DIR" "$LATCHBOARD_STATE_DIR" "$LATCHBOARD_CONFIG"
printf "ab1001\n" > "$LATCHD_CONFIG/nodes"

"$ROOT_DIR/target/debug/latchd" claim ab1001 --holder loop-dash -p 7

cargo build --manifest-path="$DASH_DIR/Cargo.toml" --quiet
DASH_BIN="$DASH_DIR/target/debug/latchboard-dashboard"

"$DASH_BIN" --once | grep -q "ab1001 -> loop-dash"
