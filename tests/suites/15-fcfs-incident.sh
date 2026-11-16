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
export LATCH_NOTE_BIN="$LATCH_NOTE"
export LATCH_NOTE_NO_NOTIFY=1
export LATCH_NOTE_NO_PROCS=1
unset LATCH_SESSION LATCH_ROLE

mkdir -p "$LATCHD_DIR" "$LATCH_HANDOVER_DIR" "$LATCHD_CONFIG"
printf "n1\nn2\nn3\nn4\nn5\n" > "$LATCHD_CONFIG/nodes"
DB="$LATCHD_DIR/claims.db"

# a command that must fail, with its message checked
refused() { local pattern="$1"; shift; set +e; local out; out="$("$@" 2>&1)"; local rc=$?; set -e
    [ "$rc" -ne 0 ] || { echo "expected failure: $*" >&2; return 1; }
    echo "$out" | grep -q -- "$pattern" || { echo "expected '$pattern' in: $out" >&2; return 1; }; }
inbox() { "$LATCH_NOTE" inbox "$1" --pop; }

# ---------------------------------------------------------------- FCFS queue
"$LATCHD" claim n1 --holder tick-a -p 5 | grep -q "claimed n1 for tick-a (p5)"
refused "refused: n1 is held by tick-a (p5); run \`latchd wait n1\` to queue and ask for it" \
    "$LATCHD" claim n1 --holder tick-b -p 9

# first come, first served: priority does not reorder the line
"$LATCHD" wait n1 --waiter tick-b -p 3 --reason "disk check" \
    | grep -q "queued tick-b for n1 (1st in line); asked tick-a to hand it over; it has 15m to \`latchd reply\`"
sqlite3 "$DB" "UPDATE queue SET enqueued_at = enqueued_at - 60 WHERE waiter = 'tick-b';"
"$LATCHD" wait n1 --waiter tick-c -p 9 | grep -q "queued tick-c for n1 (2nd in line); held by tick-a; tick-b is ahead of you"
refused "tick-a already holds n1" "$LATCHD" wait n1 --waiter tick-a

# waiting again keeps the place in line and does not ask twice
"$LATCHD" wait n1 --waiter tick-b | grep -q "(1st in line); already asked tick-a 0m ago"
[ "$(inbox tick-a | grep -c "tick-b is waiting for n1 (disk check)")" -eq 1 ]

# the holder releases: the first in line is offered the node, nobody else may claim it
"$LATCHD" release n1 --holder tick-a | grep -q "released n1 (tick-b is next in line and was told)"
inbox tick-b | grep -q "n1 is free and you are first in line"
refused "refused: n1 is free but tick-b is first in line" "$LATCHD" claim n1 --holder tick-c -p 9
refused "refused: n1 is free but tick-b is first in line" "$LATCHD" claim n1 --holder tick-z
"$LATCHD" claim n1 --holder tick-b -p 3 | grep -q "claimed n1 for tick-b (p3)"

# the next in line now asks the new holder
"$LATCHD" sweep | grep -q "asked tick-b to hand n1 over to tick-c"

# ------------------------------------------------------ takeover negotiation
# no: the waiter is told and stays first in line
"$LATCHD" reply n1 no --eta 1h --holder tick-b | grep -q "told tick-c that n1 stays with you for about 1h00m"
inbox tick-c | grep -q "tick-b will not hand n1 over (done in about 1h00m); you stay first in line"
"$LATCHD" wait n1 --waiter tick-c | grep -q "tick-b declined; it expects to be done in about"
"$LATCHD" status | grep -q "n1 waiting by tick-c (1 in line, .*, tick-b declined)"
# ...and is not asked again until the ETA has passed
if "$LATCHD" sweep | grep -q "asked tick-b"; then exit 1; fi
sqlite3 "$DB" "UPDATE queue SET eta_ts = eta_ts - 3700, replied_at = replied_at - 3700 WHERE waiter = 'tick-c';"
"$LATCHD" sweep | grep -q "asked tick-b to hand n1 over to tick-c"

# yes with an ETA is a promise: the holder keeps the node for now
"$LATCHD" reply n1 yes --eta 20m --holder tick-b | grep -q "told tick-c it gets n1 in about 20m"
"$LATCHD" status | grep -q "n1 held by tick-b"
"$LATCHD" status | grep -q "tick-b promised it in"
inbox tick-c | grep -q "tick-b will hand n1 over in about 20m"

# only the holder may answer, and only when someone is asking
refused "n1 is held by tick-b, not tick-x" "$LATCHD" reply n1 yes --holder tick-x
refused "nobody is asking for n2" bash -c "'$LATCHD' claim n2 --holder tick-a >/dev/null && '$LATCHD' reply n2 yes --holder tick-a"
refused "invalid --eta" "$LATCHD" reply n1 yes --eta soon --holder tick-b

# a plain yes hands the node over on the spot
"$LATCHD" reply n1 yes --holder tick-b | grep -q "handed n1 over to tick-c"
"$LATCHD" status | grep -q "n1 held by tick-c (p9)"
inbox tick-c | grep -q "tick-b handed n1 over to you"
[ "$(sqlite3 "$DB" "SELECT COUNT(*) FROM queue WHERE node = 'n1';")" -eq 0 ]

# ------------------------------------------- silence: the operator is asked
"$LATCHD" wait n2 --waiter tick-b --reason "firmware" | grep -q "asked tick-a to hand it over"
if "$LATCHD" sweep | grep -q "operator"; then exit 1; fi
sqlite3 "$DB" "UPDATE queue SET asked_at = asked_at - 901 WHERE node = 'n2';"
"$LATCHD" sweep | grep -q "asked the operator about n2: tick-b is waiting on tick-a$"
python3 - "$LATCH_HANDOVER_DIR/asks.json" << 'PY'
import json, sys
asks = json.load(open(sys.argv[1]))
assert len(asks) == 1, asks
q = asks[0]
assert q["tick_id"] == "tick-b" and q["nodes"] == ["n2"] and q["answered_at"] is None, q
assert "tick-b asked tick-a for n2 (firmware) 15m ago and got no reply" in q["question"], q
PY
"$LATCHD" status | grep -q "n2 waiting by tick-b (1 in line, .*, tick-a silent: operator asked)"
# asked once, not on every sweep
if "$LATCHD" sweep | grep -q "operator"; then exit 1; fi
grep -q '"takeover_escalated"' "$LATCHD_DIR/events.jsonl"

# a promise that is not kept also reaches the operator
"$LATCHD" reply n2 yes --eta 10m --holder tick-a > /dev/null
sqlite3 "$DB" "UPDATE queue SET eta_ts = eta_ts - 1600 WHERE node = 'n2';"
"$LATCHD" sweep | grep -q "asked the operator about n2"
grep -q "tick-a promised n2 to tick-b (firmware) but still holds it" "$LATCH_HANDOVER_DIR/asks.json"

# ------------------------------------------- a waiter that lets its turn pass
"$LATCHD" wait n2 --waiter tick-d > /dev/null
"$LATCHD" release n2 --holder tick-a | grep -q "tick-b is next in line"
sqlite3 "$DB" "UPDATE queue SET offered_at = offered_at - 601 WHERE node = 'n2' AND waiter = 'tick-b';"
SWEEP="$("$LATCHD" sweep)"
echo "$SWEEP" | grep -q "tick-b let its turn for n2 pass and left the queue"
echo "$SWEEP" | grep -q "offered free node n2 to tick-d"
inbox tick-b | grep -q "you did not claim n2 within 10m"
"$LATCHD" claim n2 --holder tick-d | grep -q "claimed n2 for tick-d"

# --------------------------------------------------- incident tick supremacy
# an incident tick takes a host from a standard tick without asking; the queue is bypassed
"$LATCHD" claim n3 --holder tick-a > /dev/null
"$LATCHD" wait n3 --waiter tick-b > /dev/null
inbox tick-a > /dev/null
"$LATCHD" grant-take n3 --session incident-1 > /dev/null
"$LATCHD" take n3 --session incident-1 | grep -q "incident take: n3 claimed by incident-1 at p9 (taken from tick-a)"
inbox tick-a | grep -q "incident tick incident-1 took n3 from you: stop working on it"
"$LATCHD" status | grep -q "n3 held by incident-1 (p9) \[incident: off-limits\]"

# its hosts are off-limits: no claim, no takeover question, no take by another incident tick
refused "refused: n3 is held by incident tick incident-1 and is off-limits" "$LATCHD" claim n3 --holder tick-a -p 9
"$LATCHD" wait n3 --waiter tick-c | grep -q "held by incident tick incident-1, which is never asked to yield"
if "$LATCHD" sweep | grep -q "n3"; then exit 1; fi
sqlite3 "$DB" "UPDATE queue SET enqueued_at = enqueued_at - 7200 WHERE node = 'n3';"
if "$LATCHD" sweep | grep -q "n3"; then exit 1; fi
[ -z "$(inbox incident-1)" ]
refused "nobody is asking for n3" "$LATCHD" reply n3 yes --holder incident-1
"$LATCHD" grant-take n3 --session incident-2 > /dev/null
refused "refused: n3 is held by incident tick incident-1 and is off-limits" "$LATCHD" take n3 --session incident-2

# several incident ticks work side by side, each on its own hosts
"$LATCHD" grant-take n4 --session incident-2 > /dev/null
"$LATCHD" take n4 --session incident-2 | grep -q "incident take: n4 claimed by incident-2 at p9$"
LATCH_ROLE=incident "$LATCHD" claim n5 --holder incident-2 -p 9 | grep -q "claimed n5 for incident-2 (p9) \[incident\]"
[ "$("$LATCHD" status | grep -c "\[incident: off-limits\]")" -eq 3 ]
refused "off-limits" "$LATCHD" claim n5 --holder incident-1 --incident
# renewing its own host keeps it an incident claim
"$LATCHD" claim n3 --holder incident-1 | grep -q "renewed n3"
"$LATCHD" status | grep -q "n3 held by incident-1 (p9) \[incident: off-limits\]"

# a free host with a queue: an incident tick does not wait in line, a standard one does
"$LATCHD" claim n5 --holder incident-2 > /dev/null
"$LATCHD" release n5 --holder incident-2 > /dev/null
"$LATCHD" wait n5 --waiter tick-a | grep -q "n5 is free: run \`latchd claim n5\` now"
refused "tick-a is first in line" "$LATCHD" claim n5 --holder tick-b
"$LATCHD" claim n5 --holder incident-1 --incident | grep -q "claimed n5 for incident-1"

# once the incident tick releases, the line moves again
"$LATCHD" release n3 --holder incident-1 | grep -q "released n3 (tick-b is next in line and was told)"
"$LATCHD" claim n3 --holder tick-b | grep -q "claimed n3 for tick-b"
"$LATCHD" sweep | grep -q "asked tick-b to hand n3 over to tick-c"

# status --json carries the queue position and the incident flag
"$LATCHD" status --json | python3 -c '
import json, sys
d = json.load(sys.stdin)
claims = {c["node"]: c for c in d["claims"]}
assert claims["n4"]["incident"] is True and claims["n3"]["incident"] is False, claims
q = [x for x in d["queue"] if x["node"] == "n3"]
assert [(x["waiter"], x["position"]) for x in q] == [("tick-c", 1)], q
assert q[0]["asked_holder"] == "tick-b", q
'

# a database from before this change is upgraded in place
OLD="$SCRATCH/old"
mkdir -p "$OLD"
sqlite3 "$OLD/claims.db" "
CREATE TABLE claims (node TEXT PRIMARY KEY, holder TEXT NOT NULL, prio INTEGER NOT NULL, claimed_at INTEGER NOT NULL, last_used_at INTEGER NOT NULL, warned_at INTEGER, known_node INTEGER NOT NULL DEFAULT 1);
CREATE TABLE queue (node TEXT NOT NULL, waiter TEXT NOT NULL, base_prio INTEGER NOT NULL, enqueued_at INTEGER NOT NULL, PRIMARY KEY (node, waiter));
INSERT INTO claims VALUES ('n1', 'tick-a', 5, strftime('%s','now'), strftime('%s','now'), NULL, 1);
INSERT INTO queue VALUES ('n1', 'tick-b', 5, strftime('%s','now') - 300);
"
LATCHD_DIR="$OLD" "$LATCHD" status | grep -q "n1 held by tick-a (p5)$"
LATCHD_DIR="$OLD" "$LATCHD" status | grep -q "n1 waiting by tick-b (1 in line, 5m)"
LATCHD_DIR="$OLD" "$LATCHD" sweep | grep -q "asked tick-a to hand n1 over to tick-b"
