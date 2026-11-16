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
  `warn` (print a warning) and `pause` (refuse a paused tick or all ticks).
  Node patterns are an exact name, `prefix*`, `*` or `all`.
- **Known nodes:** names listed in the `nodes` file under `LATCHD_CONFIG` or
  `LATCHBOARD_CONFIG`. A claim on an unknown node is allowed but marked, and the
  sweeper releases it after 10 idle minutes.
- **Queue (first come, first served):** `latchd wait <node> [--reason TEXT]`
  joins the node's queue; order is arrival order and priority plays no part.
  A free node with a queue can only be claimed by the first in line.
- **Takeover negotiation:** a standard tick never takes a held node. The first
  in line asks the holder (a note in the holder's inbox), and the holder
  answers with `latchd reply <node> yes` (the claim moves to the waiter at
  once), `yes --eta 20m` (a promise; the holder keeps the node until it
  releases it) or `no [--eta 1h]` (the waiter is asked again after the ETA, or
  after 15 minutes without one). Answers reach the waiter's inbox.
- **Sweep (`latchd sweep`, or `latchd daemon` every 15 s):** an idle claim is
  warned after 45 minutes and released 10 minutes after the warning. A claim
  is skipped while its holder has a non-expired heartbeat that lists the node
  (`latch-note heartbeat` writes `$LATCH_HANDOVER_DIR/peers/<tick>.json`, valid
  for twice the tick interval). The sweep also runs the queues: it offers a
  freed node to the first in line and drops a waiter that does not claim it
  within 10 minutes, sends pending takeover questions, and when a holder has
  not answered for 15 minutes (or is 15 minutes past a promised ETA) asks the
  operator once through `latch-note ask`.
  The sweep also removes `prio_floor` and synced directives whose end time has
  passed (logged as `directive_expired`).
- **Incident ticks:** `latchd grant-take` records that the operator named a
  node to a session; within 60 minutes `latchd take` then claims it at priority
  9 as an incident claim, taking it from a standard holder without asking
  (the old holder is told through its inbox) and bypassing the queue. Claims an
  incident tick makes with `latchd claim` are incident claims too (`--incident`,
  or `LATCH_ROLE=incident` as the runner sets it). An incident claim is
  off-limits: no other session can claim or take the node, other incident ticks
  included, and its holder is never asked to yield. Any number of incident
  ticks can run side by side, each on its own nodes.

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
- `directive sync NAME [host...] [--text T] [mode flags]`: keeps the directive
  called NAME on exactly the hosts given in this call, for a script that
  re-asserts it regularly. A host no longer given drops off at once, no hosts
  removes the directive, and without a further sync it expires 26 hours after
  the last one. `directive sync --list [--json]` shows them.
- `send`, `inbox`, `ask`, `answered`: notes between sessions and questions to the
  operator.
- `said [--tick T] [--since 2h] [--kind prompt|answer] [-n N] [--json]`:
  exports the local audit log (`said.jsonl`) of what the operator told the
  ticks: prompts submitted in a tick tab (recorded by the `prompt` hook, only
  inside a tick session, and never the prompt or `TICK` wake-ups the runner
  sends itself) and answers given with `answered`. Text is capped at 400
  characters.
- `digest <node>... [--action blame|restart|reboot|escalate] [--within 24h]
  [--max-age 24h] [--refresh] [--json]`: the pre-action check. It gathers what
  is known about maintenance on each node and exits 0 (`clear`), 1 (`caution`)
  or 2 (`hold`). Active maintenance windows and block directives hold; upstream
  notices, maintenance or directives starting within `--within`, other active
  directives, an active rollout that still has the node open, and an upstream
  cache older than `--max-age` are cautions. Each run is logged as a `digest`
  event.
- `heartbeat`: records the nodes a session is actively using so the sweeper
  leaves those claims alone.
- `rollout start | mark | phase | note | state | ls | show`: tracks multi-step
  changes.
- `nodes check`, `notices fetch`: validate the `nodes` file and pull advisories
  from the sources it lists (JSON or RSS/Atom) into `notices.json`.
- `notices ls [--node NODE]... [--json]`: list the cached notices, optionally
  only those targeting the given nodes.

### latch-run and latch-up (`crates/runner`, `crates/launcher`)

A *tick* is a Markdown file: `key: value` headers, a line containing `---`, then
the prompt. `latch-run <tick>` parses it, starts the agent command
(`LATCH_AGENT_BIN`, a `runner:` header, or `claude`) inside a PTY with
`LATCH_SESSION`, `LATCH_TITLE`, `LATCH_ROLE` and `LATCH_PRIO` set (priority 9
for `role: incident`, otherwise 5), and writes `TICK <ts>` lines to it every
`every:` interval.

It also serves a Unix-socket control interface (`tick`, `pause`, `resume`,
`restart`, `status`) and restarts the agent according to `restart_on`:
`crash` (child exited), `upgrade` (the runner binary changed on disk) and
`change` (the tick file or `common.md` changed).

`latch-up` is the operator front end: `start | stop | restart | pause | resume
| tick | status [--json]` for one tick or `all`. `latch-up sweep` (also run
before every `start`) removes the registry entry and socket of runners whose
process is gone.

### Shared crates

`latch-core` holds path and time helpers (all state locations are overridable
with environment variables) and the JSONL appender. `latch-proto` holds the
types shared by the binaries: `Directive`, `RunnerState`, the control
request/response enums, and the duration parser (`15m`, `1h`, `2d`).

### dashboard (`dashboard/`)

A separate cargo project (excluded from the workspace) that renders a ratatui
terminal view of claims, rollouts, runners, notices, directives and maintenance
windows. `--once` prints a plain snapshot instead.

A single key shows one panel full-size with `j`/`k` scrolling: `d` directives,
`o` overview, `m` maintenance, `h` host queues, `w` runners (the watch ticks),
`n` notices, plus `r` rollouts and `a` activity. The same key or `Esc` returns
to the grid. `/` opens a command line:

- `/shutdown N|<tick>|all` stops the Nth runner listed, a tick by id, or all
  of them (`latch-up stop`).
- `/close <rollout>` marks a rollout, by id or name, as done
  (`latch-note rollout state <rollout> done`).
- `/help`, `/quit`.

Commands that change something ask for `y` first, and the dashboard itself
stays read-only: every change goes through the CLI named above
(`src/app.rs` holds the key and command logic, `src/main.rs` the drawing).

## State and configuration layout

| Path | Written by | Contents |
|---|---|---|
| `$LATCHD_DIR/claims.db`, `events.jsonl` | latchd | Claims, queue, take grants, event log |
| `$LATCH_HANDOVER_DIR/directives.json` | latch-note | Active directives |
| `$LATCH_HANDOVER_DIR/peers/*.json` | latch-note | Heartbeats and session state |
| `$LATCH_HANDOVER_DIR/notices.json` | latch-note | Fetched advisories |
| `$LATCHBOARD_STATE_DIR/sockets/` | latch-run | Per-tick control sockets |
| `$LATCHBOARD_CONFIG/{nodes,ticks/,common.md}` | you | Inventory, tick files, shared rules |

## Tests

`tests/run.sh` builds the workspace and runs each `tests/suites/NN-*.sh`
against scratch directories (nothing touches your real state). Suites cover
claims, directives and maintenance windows, heartbeat sweeps, rollouts, hook
refusal and notices, the runner lifecycle, the launcher and manifest tooling,
the dashboard snapshot and its interactive commands (driven in tmux; skipped
when tmux is missing), the stale runner sweep, per-tick configuration, the
session audit log, the pre-action digest, directive sync, and the
first-come-first-served queue with takeover replies and incident ticks.
