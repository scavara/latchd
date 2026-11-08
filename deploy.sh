#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$ROOT_DIR/dashboard"

# Compile the standalone dashboard crate
cargo build --release --quiet

# Resolve installation directory
PREFIX_BIN="${LATCH_PREFIX_BIN:-$HOME/.local/bin}"
mkdir -p "$PREFIX_BIN"

# Install the binary atomically
cp -f "target/release/latchboard" "$PREFIX_BIN/latchboard"
chmod 0755 "$PREFIX_BIN/latchboard"

printf "deployed latchboard dashboard -> %s/latchboard\n" "$PREFIX_BIN"
