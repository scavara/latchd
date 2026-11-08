#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MANIFEST="$ROOT_DIR/manifest.tsv"
TARGET_HOME="${LATCH_HOME:-$HOME}"
CLAUDE_SETTINGS="${CLAUDE_SETTINGS_PATH:-$TARGET_HOME/.claude/settings.json}"

cmd="${1:-status}"

case "$cmd" in
    check)
        while IFS=$'\t' read -r src rel mode; do
            [ -z "$src" ] && continue
            if [ ! -f "$ROOT_DIR/$src" ]; then
                echo "missing source: $src" >&2
                exit 1
            fi
        done < "$MANIFEST"
        python3 -m json.tool "$ROOT_DIR/hooks/claude-hooks.json" >/dev/null
        echo "manifest and hooks ok"
        ;;
    status)
        while IFS=$'\t' read -r src rel mode; do
            [ -z "$src" ] && continue
            dst="$TARGET_HOME/$rel"
            if [ ! -f "$dst" ]; then
                printf "MISSING   %s\n" "$rel"
            elif cmp -s "$ROOT_DIR/$src" "$dst"; then
                printf "UPTODATE  %s\n" "$rel"
            else
                printf "MODIFIED  %s\n" "$rel"
            fi
        done < "$MANIFEST"
        ;;
    install)
        while IFS=$'\t' read -r src rel mode; do
            [ -z "$src" ] && continue
            dst="$TARGET_HOME/$rel"
            mkdir -p "$(dirname "$dst")"
            tmp="${dst}.new-$$"
            cp "$ROOT_DIR/$src" "$tmp"
            chmod "$mode" "$tmp"
            mv -f "$tmp" "$dst"
            printf "installed %s\n" "$dst"
        done < "$MANIFEST"
        ;;
    pull)
        while IFS=$'\t' read -r src rel mode; do
            [ -z "$src" ] && continue
            dst="$TARGET_HOME/$rel"
            if [ -f "$dst" ]; then
                cp "$dst" "$ROOT_DIR/$src"
                printf "pulled %s\n" "$src"
            fi
        done < "$MANIFEST"
        ;;
    hooks)
        mkdir -p "$(dirname "$CLAUDE_SETTINGS")"
        python3 - "$ROOT_DIR/hooks/claude-hooks.json" "$CLAUDE_SETTINGS" << 'PY'
import json, pathlib, sys
src = json.loads(pathlib.Path(sys.argv[1]).read_text())
dst_path = pathlib.Path(sys.argv[2])
dst = json.loads(dst_path.read_text()) if dst_path.exists() else {}
dst.setdefault("hooks", {}).update(src.get("hooks", {}))
dst_path.write_text(json.dumps(dst, indent=2) + "\n")
print(f"merged hooks into {dst_path}")
PY
        ;;
    *)
        echo "Usage: ./manage.sh [status|install|pull|hooks|check]" >&2
        exit 2
        ;;
esac
