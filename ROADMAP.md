# Latchboard Roadmap & TODOs

This project is under active development. The core daemon, locking engine, and TUI are functional, but several operational features are planned for the upcoming sprints.

## Upcoming Features (Next 20-30 Hours)

- [ ] **JSON Export for Rollouts:** Add a `--json` flag to `latch-note rollout show` to enable automated parsing of active rollout states by external monitoring scripts.
- [ ] **Priority-Floor Expiry Cleanup:** Implement automatic timestamp expiration and cleanup for `prio_floor` directives inside the `latchd/src/directives.rs` state engine.
- [ ] **Stale Socket Sweeps:** Add a cleanup routine in `latch-up` (the launcher) to detect and remove orphaned `.sock` files when a runner PID dies unexpectedly.
- [ ] **Notice Filtering by Node:** Expand the `latch-note notices` CLI to allow filtering cached hardware/upstream notices by specific target nodes (e.g., `latch-note notices list --node ab1001`).
- [ ] **Dashboard Color Coding:** Upgrade the read-only `ratatui` dashboard (`latchboard`) to use priority-based color coding for the active claims list, making high-priority/incident holds visually distinct.

## Core Operational Principles

- **State Independence:** The dashboard must remain strictly read-only, reading runtime state passively. Any state mutations must be delegated through established CLIs (`latch-note`, `latch-run`, `latch-up`, or `latchd`) rather than modifying runtime JSON directly.
- **Resilience:** The UI must handle missing, malformed, locked, or newer-than-known data gracefully. A single corrupt data source must not crash the entire dashboard.
- **Lifecycle Coordination:** Loop restarts must follow the supported launcher/runner lifecycle path, allowing active ticks to finish and preserving handover information, rather than aggressively terminating processes.
- **Fixture Determinism:** All tests must continue to use synthetic fixtures and backdated timestamps, overriding state directories to temporary paths instead of interacting with live local state.
