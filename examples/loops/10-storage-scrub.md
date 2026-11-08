title: Storage Scrub Loop
every: 15m
restart_on: crash, upgrade, change
agent_args: --quiet
---
Check ZFS pool health and SMART counters on storage nodes. End each tick with latch-note heartbeat.
