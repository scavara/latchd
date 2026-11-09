# latchd architecture overview

latchd has two jobs: keep concurrent agent sessions from colliding on the
same machine, and keep those sessions running on a schedule.

```
operator ──► latch-note directive / maintenance
                    │ writes directives.json
                    ▼
 agent session ──► PreToolUse hook ──► latch-note hook pre-command
 (claude, via          │                        │ finds known node names in the command
  latch-run)           │                        ▼
                       │                 latchd claim <node>  ──►  claims.db (SQLite)
                       │                        │ refused? exit 2 blocks the command
                       ▼                        ▼
                 command runs            events.jsonl
```

## Components

### latchd (`latchd/`)

Owns `claims.db` (SQLite, WAL mode) with three tables: `claims` (one holder per
node), `queue` (waiters, keyed by node and waiter) and `take_grants`.

- **Claim:** `latchd claim <node> --holder <id> -p <1-9>`. Before the database
  is touched, `directives.rs` evaluates every active directive that matches the
  node. Modes: `block` (refuse), `owner` (reserved for one holder), `min_prio`
  (refuse below a priority), `prio_floor` (raise the effective priority),
  `warn` (print a warning) and `pause` (refuse a paused loop or all loops).
  Node patterns are an exact name, `prefix*`, `*` or `all`.
- **Known nodes:** names listed in the `nodes` file under `LATCHD_CONFIG` or
  `LATCHBOARD_CONFIG`. A claim on an unknown node is allowed but marked, and the
  sweeper releases it after 10 idle minutes.
- **Queue and priority:** `latchd wait` enqueues a waiter. Effective priority
  is the base priority plus 1 per 30 minutes waited, capped at +2 (maximum 9).
- **Sweep (`latchd sweep`, or `latchd daemon` every 15 s):** an idle claim is
  warned after 45 minutes and released 10 minutes after the warning. A claim
  is skipped while its holder has a non-expired heartbeat that lists the node
  (`latch-note heartbeat` writes `$LATCH_HANDOVER_DIR/peers/<loop>.json`, valid
  for twice the tick interval). When a waiter's effective
  priority is at least the holder's plus 2, a `yield_request` event is logged.
- **Incident take:** `latchd grant-take` records that a session may take over a
  node; `latchd take` then claims it at priority 9.

Every state change is appended to `events.jsonl`.

### latch-note (`bin/latch-note`)

A single-file Python 3 CLI (standard library only). It is the glue between the
agent and the rest of the system:

- `hook pre-command | prompt | state`: the Claude Code hooks (see
  `hooks/claude-hooks.json`). `pre-command` strips read-only segments such as
  `echo` and `latch-note ...`, finds known node names in the rest of the
  command, and calls `latchd claim` for each. A refusal exits with status 2,
  which blocks the command. `prompt` records take grants when an operator
  prompt names a node. `state` writes the session's status file.
- `directive`, `maintenance`: write operator directives (`directives.json`).
- `send`, `inbox`, `ask`, `answered`: notes between sessions and questions to the
  operator.
- `heartbeat`: records the nodes a session is actively using so the sweeper
  leaves those claims alone.
- `rollout start | mark | phase | note | state | ls | show`: tracks multi-step
  changes.
- `nodes check`, `notices fetch`: validate the `nodes` file and pull advisories
  from the sources it lists (JSON or RSS/Atom) into `notices.json`.

### latch-run and latch-up (`crates/runner`, `crates/launcher`)

A *loop* is a Markdown file: `key: value` headers, a line containing `---`, then
the prompt. `latch-run <loop>` parses it, starts the agent command
(`LATCH_AGENT_BIN`, a `runner:` header, or `claude`) inside a PTY with
`LATCH_SESSION`, `LATCH_TITLE` and `LATCH_PRIO` set (priority 9 for
`role: incident`, otherwise 5), and writes `TICK <ts>` lines to it every
`every:` interval.

It also serves a Unix-socket control interface (`tick`, `pause`, `resume`,
`restart`, `status`) and restarts the agent according to `restart_on`:
`crash` (child exited), `upgrade` (the runner binary changed on disk) and
`change` (the loop file or `common.md` changed).

`latch-up` is the operator front end: `start | stop | restart | pause | resume
| tick | status [--json]` for one loop or `all`.

### Shared crates

`latch-core` holds path and time helpers (all state locations are overridable
with environment variables) and the JSONL appender. `latch-proto` holds the
types shared by the binaries: `Directive`, `RunnerState`, the control
request/response enums, and the duration parser (`15m`, `1h`, `2d`).

### dashboard (`dashboard/`)

A separate cargo project (excluded from the workspace) that renders a ratatui
terminal view of claims, rollouts, runners and notices. `--once` prints a plain
snapshot instead, which is what the test suite uses.

## State and configuration layout

| Path | Written by | Contents |
|---|---|---|
| `$LATCHD_DIR/claims.db`, `events.jsonl` | latchd | Claims, queue, take grants, event log |
| `$LATCH_HANDOVER_DIR/directives.json` | latch-note | Active directives |
| `$LATCH_HANDOVER_DIR/peers/*.json` | latch-note | Heartbeats and session state |
| `$LATCH_HANDOVER_DIR/notices.json` | latch-note | Fetched advisories |
| `$LATCHBOARD_STATE_DIR/sockets/` | latch-run | Per-loop control sockets |
| `$LATCHBOARD_CONFIG/{nodes,loops/,common.md}` | you | Inventory, loop files, shared rules |

## Tests

`tests/run.sh` builds the workspace and runs each `tests/suites/NN-*.sh`
against scratch directories (nothing touches your real state). Suites cover
claims, directives and maintenance windows, heartbeat sweeps, rollouts, hook
refusal, the runner lifecycle, the launcher and manifest tooling, and the
dashboard snapshot.
