title: Storage Scrub Tick
every: 15m
restart_on: crash, upgrade, change
---
Check ZFS pool health and SMART counters on storage nodes. End each tick with latch-note heartbeat.
