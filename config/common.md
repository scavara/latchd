# latchd: common tick rules

Every watch tick session follows these standing rules on each tick and interactive prompt:

1. **Ticks and heartbeats.** At the end of every scheduled `TICK`, publish a heartbeat with
   `latch-note heartbeat "<summary>" <nodes in progress>` so `latchd` protects active nodes
   from the idle sweep for the next two ticks.
2. **Inbox first.** At the start of each tick, check `latch-note inbox --pop` and take over
   any nodes handed over from peer ticks.
3. **Respect node locks and directives.** Never bypass a `latchd` refusal or directive block.
   If a target node is held by another session, run `latchd wait <node> --reason "<why>"`: you
   join its queue (first come, first served) and the holder is asked to hand it over. Then wait
   for the answer in your inbox, or hand the work to the holding tick with `latch-note send`.
   When your own inbox says another tick is waiting for a node you hold, answer promptly with
   `latchd reply <node> yes` (hand it over now), `latchd reply <node> yes --eta 20m` (when you
   will be done) or `latchd reply <node> no --eta 1h`. When it says a node is free and you are
   first in line, claim it within 10 minutes.
4. **Questions.** When human confirmation is required before a destructive action or maintenance
   window, run `latch-note ask "<question>" <nodes>` and emit `[needs-you]` until answered.
5. **Check the digest before acting on a host.** Before you blame a host for a fault, restart
   a service on it, reboot it, or escalate it, run
   `latch-note digest <node> --action blame|restart|reboot|escalate`. Exit status 0 (`CLEAR`)
   means go ahead. Status 1 (`CAUTION`: upstream notice, maintenance starting soon, rollout in
   progress) means do not act on your own: ask with `latch-note ask` and wait. Status 2
   (`HOLD`: maintenance or a block is active) means the fault is expected: leave the host alone
   and note it in your heartbeat. Treat any other failure of the command as `CAUTION`.
6. **Incident standby.** A tick with `role: incident` runs with no scheduled ticks, claims
   nodes at `p9`, and may only run `latchd take <node>` within 60 minutes of the user naming
   that node directly in its prompt. A take does not ask the holder and does not queue. A node
   held by an incident tick is off-limits to everyone else, other incident ticks included: if
   you are told one took a node from you, stop working on that node at once.
