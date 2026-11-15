# zsh-history-sanitize

Cleans `~/.zsh_history` on a schedule. Not part of latchd itself; it only
lives here. Python 3 standard library, plus `zsh` on `PATH`.

An entry is removed when it is:

| Reason | Meaning |
|---|---|
| `multiline` | spans more than one line (loops, heredoc bodies, pasted blocks) |
| `rm` | runs `rm`, including `sudo rm`, `\rm`, `/bin/rm`, `… \| xargs rm`, `find -exec rm`, `a && rm b`, `$(rm x)`. `git rm`, `docker rm` and `rmdir` are kept |
| `syntax` | `zsh -n` cannot parse it: unclosed quote, trailing `\|` or `&&`, `if` without `fi` |
| `unknown` | its first word is not a command, alias, function, builtin or directory under `~` in an interactive zsh (so `gti status` goes) |
| `duplicate` | the same command appears again later; the newest one is kept |

Surviving lines are written back unchanged, timestamps included. Before each
rewrite the old file is copied to `~/.local/state/zsh-history-sanitize/`
(last 7 kept), and the file is replaced atomically under zsh's own history
lock.

## Install

```bash
./install.sh
```

This copies the script to `~/.local/bin` and enables a daily systemd user
timer. Look first at what the first run will remove:

```bash
zsh-history-sanitize --dry-run
```

Cron instead of systemd:

```
17 4 * * * $HOME/.local/bin/zsh-history-sanitize
```

## Notes

- The `unknown` check asks `zsh -ic` for the names it knows, so it sees your
  aliases and functions. A command you have since uninstalled counts as
  unknown. Disable the check with `--no-unknown-check`; it is skipped on its
  own if the interactive zsh cannot be queried.
- Only the first command of a line is checked for typos (`ls | gerp x` stays).
- A shell that is already open keeps its in-memory history and may write
  removed entries back when it exits, unless `INC_APPEND_HISTORY` or
  `SHARE_HISTORY` is set.
