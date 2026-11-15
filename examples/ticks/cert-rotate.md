title: TLS Certificate Rotation Tick
every: 120m
restart_on: crash, upgrade, change
---
Check internal TLS certificate expiration dates and trigger renewal rollouts when needed. End each tick with latch-note heartbeat.
