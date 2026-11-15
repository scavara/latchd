title: Temp & Log Cleanup Tick
every: 30m
restart_on: crash, upgrade, change
---
Prune expired temporary artifacts, old build caches, and rotated logs. End each tick with latch-note heartbeat.
