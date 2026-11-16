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
export LATCH_SESSION=scrub

mkdir -p "$LATCHD_DIR" "$LATCH_HANDOVER_DIR" "$LATCHD_CONFIG"
cat << 'EOF' > "$LATCHD_CONFIG/nodes"
[storage]
ab1001
ab1002
sources = feed.json
regions = lan-east

[compute]
testhost-a
testhost-b
testhost-c
EOF
cat << 'EOF' > "$LATCHD_CONFIG/feed.json"
[{"title": "Disk shelf firmware advisory", "regions": ["lan-east"]}]
EOF

# run digest, capture output and exit status without tripping `set -e`
digest() { set +e; OUT="$("$LATCH_NOTE" digest "$@" 2>&1)"; RC=$?; set -e; }

# nothing known: clear, exit 0, and a hint that no upstream digest is cached
digest testhost-a --action reboot
[ "$RC" -eq 0 ]
echo "$OUT" | grep -q "testhost-a: CLEAR"
echo "$OUT" | grep -q "=> CLEAR: .*you may reboot"
echo "$OUT" | grep -q "no upstream notices cached"

# an upstream notice targeting the node: caution, exit 1; other nodes stay clear
"$LATCH_NOTE" notices fetch | grep -q "fetched 1 notice"
digest ab1001 --action blame
[ "$RC" -eq 1 ]
echo "$OUT" | grep -q "ab1001: CAUTION"
echo "$OUT" | grep -q "\[caution\] upstream notice \[storage\] Disk shelf firmware advisory"
echo "$OUT" | grep -q "=> CAUTION: do not blame automatically"
digest testhost-a
[ "$RC" -eq 0 ]

# maintenance starting soon: caution; outside the look-ahead it is ignored
"$LATCH_NOTE" maintenance "Switch upgrade" testhost-a --from +2h --until +3h > /dev/null
digest testhost-a --action restart
[ "$RC" -eq 1 ]
echo "$OUT" | grep -q "\[caution\] maintenance #1 starts .*: maintenance: Switch upgrade"
digest testhost-a --within 1h
[ "$RC" -eq 0 ]

# an active maintenance window: hold, exit 2
"$LATCH_NOTE" maintenance "PSU swap" testhost-b --from 0 --until +1h > /dev/null
digest testhost-b --action reboot
[ "$RC" -eq 2 ]
echo "$OUT" | grep -q "testhost-b: HOLD"
echo "$OUT" | grep -q "\[hold\] maintenance #2 active until .*: maintenance: PSU swap"
echo "$OUT" | grep -q "=> HOLD: do not reboot automatically"

# a blocking directive (glob pattern) holds; a warn directive is a caution; a prio floor is neither
"$LATCH_NOTE" directive "Resilver in progress" 'ab10*' --until +1h --block > /dev/null
digest ab1002
[ "$RC" -eq 2 ]
echo "$OUT" | grep -q "\[hold\] directive #3 active"
"$LATCH_NOTE" directive "Flaky NIC" testhost-c --until +1h --warn > /dev/null
digest testhost-c
[ "$RC" -eq 1 ]
echo "$OUT" | grep -q "\[caution\] directive #4 (warn)"

# an expired window no longer counts
python3 - "$LATCH_HANDOVER_DIR/directives.json" << 'PY'
import json, sys
path = sys.argv[1]
data = json.load(open(path))
for d in data:
    if d["id"] == 2:
        d["from_ts"] -= 7200
        d["until_ts"] -= 7200
json.dump(data, open(path, "w"), indent=2)
PY
digest testhost-b
[ "$RC" -eq 0 ]

# an active rollout that still has the node pending: caution, until the node is done
"$LATCH_NOTE" rollout start bios-update testhost-b > /dev/null
digest testhost-b --action escalate
[ "$RC" -eq 1 ]
echo "$OUT" | grep -q "\[caution\] rollout #1 bios-update \[init\] has this node as pending"
"$LATCH_NOTE" rollout mark 1 testhost-b done > /dev/null
digest testhost-b
[ "$RC" -eq 0 ]

# several nodes: per-node verdicts, overall the worst one
digest testhost-b testhost-c ab1002
[ "$RC" -eq 2 ]
echo "$OUT" | grep -q "testhost-b: CLEAR"
echo "$OUT" | grep -q "testhost-c: CAUTION"
echo "$OUT" | grep -q "ab1002: HOLD"

# a stale upstream cache is itself a reason for caution; --refresh fixes it
touch -d "3 days ago" "$LATCH_HANDOVER_DIR/notices.json"
digest testhost-b
[ "$RC" -eq 1 ]
echo "$OUT" | grep -q "\[caution\] upstream notices were last fetched 7[0-9]h ago"
digest testhost-b --refresh
[ "$RC" -eq 0 ]

# JSON output and the audit event
set +e
JSON="$("$LATCH_NOTE" digest ab1002 testhost-b --action reboot --json)"
RC=$?
set -e
[ "$RC" -eq 2 ]
echo "$JSON" | python3 -c '
import json, sys
d = json.load(sys.stdin)
assert d["verdict"] == "hold" and d["action"] == "reboot" and d["tick"] == "scrub", d
assert d["nodes"]["testhost-b"] == {"verdict": "clear", "findings": []}, d
kinds = sorted(f["kind"] for f in d["nodes"]["ab1002"]["findings"])
assert kinds == ["directive_block", "upstream_notice"], kinds
'
grep '"event": "digest"' "$LATCH_HANDOVER_DIR/events.jsonl" | tail -1 | grep -q '"verdict": "hold"'

# an unknown action is rejected (non-zero, so a tick fails closed)
digest ab1001 --action destroy
[ "$RC" -ne 0 ]
