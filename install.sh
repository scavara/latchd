#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$ROOT_DIR"

PREFIX_BIN="${LATCH_PREFIX_BIN:-$HOME/.local/bin}"
STATE_DIR="${LATCHBOARD_STATE_DIR:-$HOME/.local/state/latchboard}"
mkdir -p "$PREFIX_BIN" "$STATE_DIR"

if [ "${LATCH_SKIP_TESTS:-0}" != "1" ]; then
    "$ROOT_DIR/tests/run.sh"
fi

cargo build --workspace --release --quiet

STAMP="$(date +%Y%m%d%H%M%S)"

install_atomic() {
    local src="$1"
    local name="$2"
    local dst="$PREFIX_BIN/$name"

    if [ -f "$dst" ]; then
        cp -p "$dst" "${dst}.bak-${STAMP}"
        local backups=()
        while IFS= read -r f; do
            [ -n "$f" ] && backups+=("$f")
        done < <(printf '%s\n' "${dst}.bak-"* 2>/dev/null | grep -v '\.keep$' | sort -r)

        local kept=0
        for b in "${backups[@]}"; do
            if [ -f "${b}.keep" ]; then
                continue
            fi
            kept=$((kept + 1))
            if [ "$kept" -gt 2 ]; then
                rm -f "$b"
            fi
        done
    fi

    local tmp="${dst}.new-$$"
    cp "$src" "$tmp"
    chmod 0755 "$tmp"
    mv -f "$tmp" "$dst"
    printf "%s\t%s\t%s\n" "$name" "$dst" "$STAMP" >> "$STATE_DIR/installed.tsv"
    printf "installed %s -> %s\n" "$name" "$dst"
}

TARGET="${1:-all}"
case "$TARGET" in
    latchd)
        install_atomic "$ROOT_DIR/target/release/latchd" "latchd"
        ;;
    runner)
        install_atomic "$ROOT_DIR/target/release/latch-run" "latch-run"
        ;;
    launcher)
        install_atomic "$ROOT_DIR/target/release/latch-up" "latch-up"
        ;;
    all)
        install_atomic "$ROOT_DIR/target/release/latchd" "latchd"
        install_atomic "$ROOT_DIR/target/release/latch-run" "latch-run"
        install_atomic "$ROOT_DIR/target/release/latch-up" "latch-up"
        ;;
    *)
        echo "Usage: ./install.sh [latchd|runner|launcher|all]" >&2
        exit 2
        ;;
esac
