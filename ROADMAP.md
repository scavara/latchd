# latchd Roadmap & TODOs

This project is under active development. The core daemon, locking engine, and TUI are functional, but several operational features are planned for the upcoming sprints.

## Upcoming Features (Next 20-30 Hours)

- [x] **JSON Export for Rollouts:** Add a `--json` flag to `latch-note rollout show` to enable automated parsing of active rollout states by external monitoring scripts.
- [x] **Priority-Floor Expiry Cleanup:** Implement automatic timestamp expiration and cleanup for `prio_floor` directives inside the `latchd/src/directives.rs` state engine.
- [x] **Stale Socket Sweeps:** Add a cleanup routine in `latch-up` (the launcher) to detect and remove orphaned `.sock` files when a runner PID dies unexpectedly.
- [x] **Notice Filtering by Node:** Expand the `latch-note notices` CLI to allow filtering cached hardware/upstream notices by specific target nodes.
- [x] **Dashboard Color Coding:** Upgrade the read-only `ratatui` dashboard to use priority-based color coding for the active claims list.

## Phase 2: Advanced Coordination & UI

- [x] **TUI Slash Commands & Navigation:** Implement interactive slash commands in the dashboard (e.g., `/shutdown N|all`, `/close ro`) and single-key panel focus shortcuts (`d`, `o`, `m`, `h`, `w`, `n`).
- [ ] **FCFS Queue & Takeover Negotiation:** Refactor the `latchd` queue to operate on a First-Come-First-Served basis. Standard ticks must ask the current holder before taking over, waiting for a `latchd reply <host> yes|no --eta` response (or timing out after 15 minutes to ask the user).
- [ ] **Incident Tick Supremacy:** Allow incident ticks to automatically take hosts from standard ticks without asking. Make hosts held by incident ticks strictly off-limits (no session can take them, and incident ticks are never asked to yield). Support multiple concurrent incident ticks.
- [x] **Per-Tick Configuration:** Support `mcp_config`, `model`, and `effort` overrides per-tick in the configuration file to allow sharing resources.
- [ ] **Secure Grafana Service Integration:** Implement a local `msc-grafana` service. The service account token must be stored securely in an encrypted secrets file at rest (e.g., using `age` or `SOPS`) and unlocked at runtime via a heavily restricted master keyfile or interactive passphrase prompt.
- [x] **Session Audit Log (`latch-note said`):** Add a command to export user prompts typed in tick tabs and answers to tick questions for local auditing, logging up to 400 characters.
- [x] **Pre-Action Maintenance Checks:** Add a `latch-note digest` hook for ticks to query a generic upstream maintenance digest before allowing them to automatically blame, restart, reboot, or escalate a host.
- [x] **Directive Sync Service:** Add a `latch-note directive sync` mechanism to keep active directives tied strictly to the presence of specific hosts, expiring 26 hours after the last sync.

## Core Operational Principles

- **State Independence:** The dashboard must remain strictly read-only, reading runtime state passively. Any state mutations must be delegated through established CLIs (`latch-note`, `latch-run`, `latch-up`, or `latchd`) rather than modifying runtime JSON directly.
- **Resilience:** The UI must handle missing, malformed, locked, or newer-than-known data gracefully.
- **Lifecycle Coordination:** Tick restarts must follow the supported launcher/runner lifecycle path, allowing active ticks to finish and preserving handover information.
- **Fixture Determinism:** All tests must continue to use synthetic fixtures and backdated timestamps, overriding state directories to temporary paths.
