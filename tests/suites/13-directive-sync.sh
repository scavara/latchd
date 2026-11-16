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
export LATCHBOARD_CONFIG="$SCRATCH/config"
export LATCHD_BIN="$LATCHD"
export LATCH_NOTE_NO_NOTIFY=1
export LATCH_NOTE_NO_PROCS=1

mkdir -p "$LATCHD_DIR" "$LATCH_HANDOVER_DIR" "$LATCHD_CONFIG"
printf "ab1001\nab1002\nab1003\ntesthost-a\n" > "$LATCHD_CONFIG/nodes"
DIRECTIVES="$LATCH_HANDOVER_DIR/directives.json"

# field of the directive with a given sync name
field() { python3 -c '
import json, sys
recs = [d for d in json.load(open(sys.argv[1])) if d.get("sync_name") == sys.argv[2]]
assert len(recs) == 1, recs
print(json.dumps(recs[0][sys.argv[3]]))
' "$DIRECTIVES" "$1" "$2"; }
# shift a synced directive into the past by N seconds
backdate() { python3 -c '
import json, sys
path, name, secs = sys.argv[1], sys.argv[2], int(sys.argv[3])
data = json.load(open(path))
for d in data:
    if d.get("sync_name") == name:
        d["until_ts"] -= secs
        d["synced_at"] -= secs
json.dump(data, open(path, "w"), indent=2)
' "$DIRECTIVES" "$1" "$2"; }

"$LATCH_NOTE" directive sync --list | grep -q "no synced directives"

# an ordinary directive is never touched by syncing
"$LATCH_NOTE" directive "Manual freeze" testhost-a --until +30m > /dev/null

# a new synced directive needs a text
if "$LATCH_NOTE" directive sync decom ab1001 2>/dev/null; then
    exit 1
fi

# first sync creates it, on exactly the hosts given, expiring 26 hours later
"$LATCH_NOTE" directive sync decom ab1001 ab1002 --text "Being decommissioned" \
    | grep -q "decom: directive #2 created (block) on ab1001, ab1002; expires "
[ "$(field decom nodes)" = '["ab1001", "ab1002"]' ]
python3 - "$DIRECTIVES" << 'PY'
import json, sys, time
d = [x for x in json.load(open(sys.argv[1])) if x.get("sync_name") == "decom"][0]
assert d["until_ts"] - d["synced_at"] == 26 * 3600, d
assert abs(d["synced_at"] - time.time()) < 60, d
PY

# latchd enforces it like any other directive
if "$LATCHD" claim ab1001 --holder tick-a -p 5 2>/dev/null; then
    exit 1
fi
"$LATCHD" claim ab1003 --holder tick-a -p 5 | grep -q "claimed ab1003"

# the next sync: a host that is gone drops off at once, a new one is added, the id and text stay
backdate decom 3600
"$LATCH_NOTE" directive sync decom ab1002,ab1003 \
    | grep -q "decom: directive #2 updated (block) on ab1002, ab1003; expires .*; dropped ab1001"
[ "$(field decom nodes)" = '["ab1002", "ab1003"]' ]
[ "$(field decom text)" = '"Being decommissioned"' ]
"$LATCHD" claim ab1001 --holder tick-a -p 5 | grep -q "claimed ab1001"
if "$LATCHD" claim ab1002 --holder tick-a -p 5 2>/dev/null; then
    exit 1
fi
# ...and the expiry moved forward again
python3 - "$DIRECTIVES" << 'PY'
import json, sys, time
d = [x for x in json.load(open(sys.argv[1])) if x.get("sync_name") == "decom"][0]
assert d["until_ts"] - int(time.time()) > 25 * 3600, d
PY

# a second synced directive with another mode lives alongside
"$LATCH_NOTE" directive sync flaky testhost-a --text "Flaky NIC" --warn | grep -q "flaky: directive #3 created (warn)"
"$LATCH_NOTE" directive sync --list | grep -q "decom: directive #2 (block) on ab1002, ab1003; synced 0m ago"
[ "$("$LATCH_NOTE" directive sync --list | wc -l)" -eq 2 ]
"$LATCH_NOTE" directive sync --list --json | python3 -c '
import json, sys
assert sorted(d["sync_name"] for d in json.load(sys.stdin)) == ["decom", "flaky"]
'

# syncing with no hosts present removes the directive
"$LATCH_NOTE" directive sync flaky | grep -q "flaky: directive #3 removed (no hosts present)"
"$LATCH_NOTE" directive sync flaky | grep -q "flaky: no hosts present, nothing to sync"

# without a further sync it lapses after 26 hours: latchd stops enforcing it...
backdate decom $((26 * 3600 + 60))
"$LATCHD" claim ab1002 --holder tick-a -p 5 | grep -q "claimed ab1002"
# ...and the sweep removes the record, leaving the manual directive alone
"$LATCHD" sweep | grep -q "expired directive #2 removed"
python3 - "$DIRECTIVES" << 'PY'
import json, sys
data = json.load(open(sys.argv[1]))
assert [d["id"] for d in data] == [1], data
assert "sync_name" not in data[0], data
PY

# a later sync run also clears lapsed ones by itself
"$LATCH_NOTE" directive sync old ab1003 --text "Old" > /dev/null
backdate old $((27 * 3600))
"$LATCH_NOTE" directive sync fresh ab1001 --text "Fresh" > /dev/null
[ "$("$LATCH_NOTE" directive sync --list | wc -l)" -eq 1 ]
"$LATCH_NOTE" directive sync --list | grep -q "^fresh: "

grep -q '"event": "directive_sync"' "$LATCH_HANDOVER_DIR/events.jsonl"

# the ordinary directive command still works as before
"$LATCH_NOTE" directive "Another freeze" ab1003 --until +10m | grep -q "recorded (block) on ab1003"
