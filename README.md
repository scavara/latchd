# latchd

Node claims, priorities and agent "ticks" for a homelab, so that several
autonomous CLI agent sessions can work on the same machines without stepping
on each other.

If you run more than one agent session (for example [Claude Code](https://claude.com/claude-code))
against a shared set of hosts, two of them will eventually `ssh` into the same
box at the same time. latchd makes that a managed situation instead of a
race: a session must *claim* a node before touching it, waiters queue by
priority, idle claims are released automatically, and an operator can issue
directives (block, warn, reserve, pause) that every session obeys.

## What's in the box

| Component | Language | What it does |
|---|---|---|
| `latchd` | Rust + SQLite | Claim/queue/take-grant store, directive checks, idle sweeper (`latchd daemon`) |
| `latch-run` | Rust | Runs one tick file as an agent session in a PTY, with a control socket and restart-on-change |
| `latch-up` | Rust | Starts, stops, pauses and triggers ticks (`latch-up start all`, `latch-up status`) |
| `latch-note` | Python 3 (stdlib only) | Session notes, heartbeats, directives, maintenance windows, rollouts, advisory feeds, the operator audit log (`latch-note said`), the pre-action maintenance check (`latch-note digest`), and the Claude Code hooks |
| `dashboard/` (`latchboard`) | Rust + ratatui | Multi-panel terminal dashboard of claims, rollouts, runners, notices, host queues and directives (`--once` for plain text) |
| `start-ticks.sh` | bash + tmux | Orchestrates all active ticks and `latchboard` in a top-tabbed `tmux` session (`--restart`, `--stop`, `--status`) |

See [OVERVIEW.md](OVERVIEW.md) for how the pieces fit together.

## Requirements

- A Unix-like OS (uses Unix sockets, PTYs and `kill(2)`)
- Rust stable, **1.78 or newer** (the lockfiles are format version 4)
- Python 3.9+ (standard library only)
- bash and `tmux`

## Build, test, install

```bash
./tests/run.sh        # builds the workspace and runs tests/suites/*.sh
./install.sh all      # release build -> ~/.local/bin (latchd, latch-run, latch-up)
./manage.sh install   # copies latch-note, config/common.md and config/config into place
./manage.sh hooks     # merges hooks/claude-hooks.json into ~/.claude/settings.json
./deploy.sh           # builds and installs the full suite + latchboard + start-ticks.sh
```

Or run the full test, release build, and deployment pipeline in one line:

```bash
cargo test && ./tests/run.sh && cargo build --release && ./deploy.sh
```

`install.sh` runs the tests first; set `LATCH_SKIP_TESTS=1` to skip them.
Existing binaries are backed up as `<name>.bak-<timestamp>` (the two newest are
kept; `touch <backup>.keep` pins one). The dashboard is a separate cargo
project: see [INSTALL.md](INSTALL.md).

## Quick tour

```bash
# Claim a node at priority 5, see who holds what
latchd claim ab1001 --holder tick-a -p 5
latchd status

# A higher-priority session queues behind it; the sweeper logs a yield request
latchd wait ab1001 --waiter tick-b -p 8
latchd sweep

# Operator directive: block every node matching ab10* for the next hour
latch-note directive "ZFS resilver in progress" 'ab10*' --until +1h --block
```

Claims are idle-swept: warned after 45 minutes without use, released 10
minutes after the warning. A queued waiter gains +1 effective priority per
30 minutes (capped at +2), and a yield is requested when a waiter outranks the
holder by 2 or more.

## Ticks

A tick is a Markdown file with `key: value` headers, a `---` line, then the
prompt the agent receives on each tick:

```
title: Storage Scrub Tick
every: 15m
restart_on: crash, upgrade, change
---
Check ZFS pool health and SMART counters on storage nodes.
```

Samples are in [`examples/ticks/`](examples/ticks/). The agent command defaults to
`claude`; set `LATCH_AGENT_BIN` (or a `runner:` header) to use another CLI.
Shared rules for every tick live in `~/.config/latchd/common.md`
(source: [`config/common.md`](config/common.md)); editing it restarts running
ticks that opted into `change`.

To launch all ticks in background tabs alongside the interactive `latchboard` TUI:

```bash
start-ticks.sh            # start or attach to the latchd-ticks tmux session
start-ticks.sh --status   # list active tick windows
start-ticks.sh --restart  # cleanly restart all runners and the tmux session
start-ticks.sh --stop     # stop all runners and clean up sockets/state
```

## Configuration

`~/.config/latchd/config` (source: [`config/config`](config/config), installed
by `./manage.sh install`) holds `key = value` settings the runner applies to
every tick it starts:

| Key | Example | Effect |
|---|---|---|
| `agent_model` | `fable` | Passed to the agent as `--model` (`fable` is the latest-model alias) |
| `agent_effort` | `low` | Passed as `--effort`; the agent's own default is `medium` |
| `agent_fullscreen` | `true` | Starts the agent TUI in fullscreen (sets `CLAUDE_CODE_NO_FLICKER=1`) |
| `agent_mcp_config` | `mcp/shared.json` | Passed as `--mcp-config`: one or more JSON files, comma or space separated |

A `[tick]` section overrides these for one tick. The section name is the tick
id or its file name without `.md`, and inside it the `agent_` prefix is
optional:

```
agent_model = fable
agent_effort = low

[incident]
model = opus
effort = high
mcp_config = mcp/incident.json
```

Relative `mcp_config` paths are relative to the config file, `~/` is expanded,
and a file that does not exist is skipped with a warning. Several ticks can
name the same file to share MCP servers.

A tick's `agent_args:` header wins over both when it sets the same flag. Editing the file
restarts running ticks that opted into `change`. `LATCH_CONFIG_FILE` points the
runner at a different file.

| Variable | Default | Purpose |
|---|---|---|
| `LATCHD_DIR` | `~/.local/state/latchd` | `claims.db` and `events.jsonl` |
| `LATCHBOARD_STATE_DIR` | `~/.local/state/latchboard` | Runner sockets and state |
| `LATCH_HANDOVER_DIR` | `~/.local/state/latchd-up` | Directives, peers, notices |
| `LATCHBOARD_CONFIG` / `LATCHD_CONFIG` | `~/.config/latchd` / `~/.config/latchd` | `nodes`, `ticks/`, `common.md` |
| `LATCH_AGENT_BIN` | `claude` | Agent executable the runner launches |

## Status

Early (`latchd` 0.3.x).

## License

[MIT](LICENSE)

## Dashboard Overview

![Dashboard Preview](docs/assets/dashboard-preview.jpg)

## Session Management

![Tmux Session Preview](docs/assets/tmux-preview.jpg)
