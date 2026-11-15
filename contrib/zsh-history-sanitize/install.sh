#!/usr/bin/env bash
# Install zsh-history-sanitize and enable its systemd user timer.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
units="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"

install -Dm755 "$here/zsh-history-sanitize" "$HOME/.local/bin/zsh-history-sanitize"
install -Dm644 -t "$units" "$here/zsh-history-sanitize.service" "$here/zsh-history-sanitize.timer"
systemctl --user daemon-reload
systemctl --user enable --now zsh-history-sanitize.timer
systemctl --user list-timers zsh-history-sanitize.timer --no-pager
