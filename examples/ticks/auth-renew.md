title: Auth Token Renewal Tick
every: 60m
restart_on: crash, upgrade, change
---
Verify OAuth and service account token TTLs across nodes and refresh expiring credentials. End each tick with latch-note heartbeat.
