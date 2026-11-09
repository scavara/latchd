# latchd: installation guide for agents

1. Verify environment prerequisites: Python 3.9+ (stdlib only), Rust 1.78+ (required for lockfile v4).
2. Run test suite: `./tests/run.sh`
3. Build and install core binaries: `./install.sh all`
4. Deploy TUI dashboard: `./deploy.sh`
5. Register Claude settings hooks: `./manage.sh hooks`
