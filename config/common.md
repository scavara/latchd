# latchboard shared rules

These rules apply to every loop. `manage.sh install` copies this file to
`~/.config/latchboard/common.md`; `latch-run` watches it and restarts loops
whose `restart_on` includes `change` when it is edited.

- Before touching a node, claim it by running the command normally. The
  pre-command hook claims it for you and refuses the command if another
  session holds it. If refused, do not look for a way around it. Wait or
  report to the operator.
- End every tick with `latch-note heartbeat "<summary>" <nodes...>`, listing
  the nodes you are still using, so those claims are not swept as idle.
- Honor operator directives. A `block`, `owner` or `pause` refusal is final
  until the directive expires.
- Only act on nodes explicitly named in your loop file or by the operator.
- Never print or store credentials, tokens or private keys in notes, events
  or loop output.
