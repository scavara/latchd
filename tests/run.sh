#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

cargo build --workspace --quiet

PASS=0
FAIL=0

for suite in "$ROOT_DIR"/tests/suites/*.sh; do
    [ -e "$suite" ] || continue
    name="$(basename "$suite")"
    if bash "$suite" "$ROOT_DIR"; then
        printf "PASS  %s\n" "$name"
        PASS=$((PASS + 1))
    else
        printf "FAIL  %s\n" "$name" >&2
        FAIL=$((FAIL + 1))
    fi
done

printf "\nSummary: %d passed, %d failed\n" "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
