# latchboard: installation guide

1. Verify prerequisites: Python 3.9+ (stdlib only) and a Rust stable toolchain,
   version 1.78 or newer (the lockfiles use format version 4).
2. Run the test suite: `./tests/run.sh`
3. Build and install the core binaries: `./install.sh all`
   (installs `latchd`, `latch-run` and `latch-up` to `~/.local/bin`;
   set `LATCH_PREFIX_BIN` to change the destination, `LATCH_SKIP_TESTS=1` to
   skip the test run).
4. Install `latch-note` and the shared rules file: `./manage.sh install`
   (`./manage.sh status` shows what is missing or modified).
5. Optional: build the TUI dashboard. It is a separate cargo project:
   `cargo build --release --manifest-path dashboard/Cargo.toml`
   then run `dashboard/target/release/latchboard-dashboard` (or add `--once`
   for a plain-text snapshot).
6. Register the Claude Code hooks: `./manage.sh hooks`
   (merges `hooks/claude-hooks.json` into `~/.claude/settings.json`).

Make sure `~/.local/bin` is on your `PATH`: the hooks call `latch-note`.
