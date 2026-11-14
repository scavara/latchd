title: Node Telemetry Sweep Tick
every: 5m
restart_on: crash, upgrade, change
agent_args: --quiet
---
Collect host resource utilization, systemd unit health, and latency metrics. End each tick with latch-note heartbeat.
