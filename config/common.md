# latchboard: common loop rules

Every watch loop session follows these standing rules on each tick and interactive prompt:

1. **Ticks and heartbeats.** At the end of every scheduled `TICK`, publish a heartbeat with
   `latch-note heartbeat "<summary>" <nodes in progress>` so `latchd` protects active nodes
   from the idle sweep for the next two ticks.
2. **Inbox first.** At the start of each tick, check `latch-note inbox --pop` and take over
   any nodes handed over from peer loops.
3. **Respect node locks and directives.** Never bypass a `latchd` refusal or directive block.
   If a target node is held by another session, run `latchd wait <node> -p <prio>` or hand the
   work to the holding loop with `latch-note send`.
4. **Questions.** When human confirmation is required before a destructive action or maintenance
   window, run `latch-note ask "<question>" <nodes>` and emit `[needs-you]` until answered.
5. **Incident standby.** A loop with `role: incident` runs with no scheduled ticks, claims
   nodes at `p9`, and may only run `latchd take <node>` within 60 minutes of the user naming
   that node directly in its prompt.
