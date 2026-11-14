#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PREFIX_BIN="${LATCH_PREFIX_BIN:-$HOME/.local/bin}"
mkdir -p "$PREFIX_BIN"

# 1. Compile and install the core workspace binaries
cd "$ROOT_DIR"
cargo build --release --quiet
for bin in latchd latch-run latch-up latch-note; do
    if [ -f "target/release/$bin" ]; then
        cp -f "target/release/$bin" "$PREFIX_BIN/$bin"
        chmod 0755 "$PREFIX_BIN/$bin"
    elif [ -f "scripts/$bin" ]; then
        cp -f "scripts/$bin" "$PREFIX_BIN/$bin"
        chmod 0755 "$PREFIX_BIN/$bin"
    elif [ -f "$bin" ]; then
        cp -f "$bin" "$PREFIX_BIN/$bin"
        chmod 0755 "$PREFIX_BIN/$bin"
    fi
done

# 2. Compile and install the standalone dashboard crate
cd "$ROOT_DIR/dashboard"
cargo build --release --quiet
cp -f "target/release/latchboard-dashboard" "$PREFIX_BIN/latchboard"
chmod 0755 "$PREFIX_BIN/latchboard"

# 3. Install the orchestrator script
if [ -f "$ROOT_DIR/start-ticks.sh" ]; then
    cp -f "$ROOT_DIR/start-ticks.sh" "$PREFIX_BIN/start-ticks.sh"
    chmod 0755 "$PREFIX_BIN/start-ticks.sh"
fi

printf "Successfully deployed latchd suite to %s\n" "$PREFIX_BIN"
