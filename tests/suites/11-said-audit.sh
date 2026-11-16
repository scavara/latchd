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
unset LATCH_SESSION LATCH_TICK_FILE

mkdir -p "$LATCHD_DIR" "$LATCH_HANDOVER_DIR" "$LATCHD_CONFIG"
printf "ab1001\n" > "$LATCHD_CONFIG/nodes"

SAID="$LATCH_HANDOVER_DIR/said.jsonl"
count() { if [ -f "$SAID" ]; then wc -l < "$SAID"; else echo 0; fi; }

"$LATCH_NOTE" said | grep -q "nothing said"

# a prompt submitted in a tick session is recorded
printf '{"prompt": "check the scrub on ab1001", "hook_event_name": "UserPromptSubmit"}' \
    | LATCH_SESSION=scrub "$LATCH_NOTE" hook prompt
[ "$(count)" -eq 1 ]
[ "$(stat -c %a "$SAID")" = "600" ]

# outside a tick session nothing is recorded
printf '{"prompt": "unrelated work in another session"}' | "$LATCH_NOTE" hook prompt
[ "$(count)" -eq 1 ]

# what the runner sends by itself is not something the operator said
cat << 'EOF' > "$SCRATCH/10-scrub.md"
title: Scrub Tick
every: 10m
---
Check pool health.
End each tick with a heartbeat.
EOF
export LATCH_TICK_FILE="$SCRATCH/10-scrub.md"
printf '{"prompt": "TICK 1763200000"}' | LATCH_SESSION=scrub "$LATCH_NOTE" hook prompt
printf '{"prompt": "Check pool health.\\nEnd each tick with a heartbeat."}' | LATCH_SESSION=scrub "$LATCH_NOTE" hook prompt
printf '{"prompt": "from a peer", "source": "peer"}' | LATCH_SESSION=scrub "$LATCH_NOTE" hook prompt
[ "$(count)" -eq 1 ]
printf '{"prompt": "Check pool health. Also look at ab1001."}' | LATCH_SESSION=scrub "$LATCH_NOTE" hook prompt
[ "$(count)" -eq 2 ]
unset LATCH_TICK_FILE

# long prompts are cut at 400 characters and marked
LONG="$(printf 'x%.0s' $(seq 1 500))"
printf '{"prompt": "%s"}' "$LONG" | "$LATCH_NOTE" hook prompt --session incident
python3 - "$SAID" << 'PY'
import json, sys
recs = [json.loads(l) for l in open(sys.argv[1])]
last = recs[-1]
assert last["tick"] == "incident" and last["kind"] == "prompt", last
assert len(last["text"]) == 400 and last["truncated"] is True and last["chars"] == 500, last
assert "truncated" not in recs[0], recs[0]
PY

# answers to tick questions are recorded against the tick that asked
LATCH_SESSION=scrub "$LATCH_NOTE" ask "Approve BIOS flash on ab1001?" ab1001 | grep -q "question #1"
"$LATCH_NOTE" answered 1 "approved, go ahead" | grep -q "question #1 marked answered"
[ "$(count)" -eq 4 ]

OUT="$("$LATCH_NOTE" said)"
echo "$OUT" | grep -q "scrub  prompt: check the scrub on ab1001"
echo "$OUT" | grep -q "incident  prompt: x.* \[\.\.\. 500 chars\]"
echo "$OUT" | grep -q "scrub  answer to #1: approved, go ahead  (Q: Approve BIOS flash on ab1001?)"
[ "$(echo "$OUT" | wc -l)" -eq 4 ]

# filters
[ "$("$LATCH_NOTE" said --tick scrub | wc -l)" -eq 3 ]
[ "$("$LATCH_NOTE" said --kind answer | wc -l)" -eq 1 ]
[ "$("$LATCH_NOTE" said --kind prompt --tick incident | wc -l)" -eq 1 ]
"$LATCH_NOTE" said -n 1 | grep -q "answer to #1"
"$LATCH_NOTE" said --tick nobody | grep -q "nothing said"

# --since: backdate the first record by two hours
python3 - "$SAID" << 'PY'
import json, sys
path = sys.argv[1]
recs = [json.loads(l) for l in open(path)]
recs[0]["ts"] -= 7200
open(path, "w").write("".join(json.dumps(r) + "\n" for r in recs))
PY
[ "$("$LATCH_NOTE" said --since 1h | wc -l)" -eq 3 ]
[ "$("$LATCH_NOTE" said --since 3h | wc -l)" -eq 4 ]

# JSON export, and a damaged line does not break it
echo "not json" >> "$SAID"
"$LATCH_NOTE" said --json --kind answer | python3 -c '
import json, sys
recs = json.load(sys.stdin)
assert len(recs) == 1 and recs[0]["question_id"] == 1 and recs[0]["text"] == "approved, go ahead", recs
'
