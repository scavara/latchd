#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$1"
LATCHD="$ROOT_DIR/target/debug/latchd"
LATCH_NOTE="$ROOT_DIR/bin/latch-note"

SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT

export LATCHD_DIR="$SCRATCH/latchd"
export LATCH_HANDOVER_DIR="$SCRATCH/handover"
export LATCHD_CONFIG="$SCRATCH/config"
export LATCH_NOTE_NO_NOTIFY=1
export LATCH_NOTE_NO_PROCS=1

mkdir -p "$LATCHD_DIR" "$LATCH_HANDOVER_DIR" "$LATCHD_CONFIG"
printf "ab1001\nab1002\ntesthost-a\n" > "$LATCHD_CONFIG/nodes"

"$LATCH_NOTE" directive "Freeze ab1001 for firmware" ab1001 --until +30m | grep -q "directive #1"

if "$LATCHD" claim ab1001 --holder tick-a -p 5 2>/dev/null; then
    exit 1
fi

"$LATCH_NOTE" directive "High prio only on ab1002" ab1002 --min-prio 7 --until +30m | grep -q "directive #2"
if "$LATCHD" claim ab1002 --holder tick-a -p 5 2>/dev/null; then
    exit 1
fi
"$LATCHD" claim ab1002 --holder tick-a -p 8 | grep -q "claimed ab1002"

"$LATCH_NOTE" directive "Elevate testhost*" "testhost*" --prio-floor 8 --until +30m | grep -q "directive #3"
"$LATCHD" claim testhost-a --holder tick-b -p 4 | grep -q "claimed testhost-a for tick-b (p8)"

"$LATCH_NOTE" maintenance "Switch upgrade" ab1002 --from +10m --until +40m | grep -q "maintenance #4"
"$LATCHD" claim ab1002 --holder tick-a -p 8 | grep -q "renewed ab1002"

# expired prio_floor directives are removed by the sweep; everything else stays
python3 - "$LATCH_HANDOVER_DIR/directives.json" << 'PY'
import json, sys
path = sys.argv[1]
data = json.load(open(path))
for d in data:
    if d["id"] in (1, 3):
        d["until_ts"] -= 3600
json.dump(data, open(path, "w"), indent=2)
PY
"$LATCHD" sweep | grep -q "expired directive #3 removed"
python3 - "$LATCH_HANDOVER_DIR/directives.json" << 'PY'
import json, sys
ids = [d["id"] for d in json.load(open(sys.argv[1]))]
assert ids == [1, 2, 4], ids
PY
grep -q '"directive_expired"' "$LATCHD_DIR/events.jsonl"
if "$LATCHD" sweep | grep -q "expired directive"; then
    exit 1
fi
"$LATCHD" release testhost-a --holder tick-b > /dev/null
"$LATCHD" claim testhost-a --holder tick-c -p 4 | grep -q "claimed testhost-a for tick-c (p4)"
