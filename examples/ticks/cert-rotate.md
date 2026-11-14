title: TLS Certificate Rotation Tick
every: 120m
restart_on: crash, upgrade, change
agent_args: --quiet
---
Check internal TLS certificate expiration dates and trigger renewal rollouts when needed. End each tick with latch-note heartbeat.
