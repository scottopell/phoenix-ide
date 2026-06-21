---
title: Use the terminal
summary: Open the in-app terminal, run commands that persist across restarts, and turn on shell integration for the status HUD.
category: howto
keywords: [terminal, shell, tmux, persistent, shell integration]
related: [reference/tools/tmux.md, concepts/workspace.md, reference/tools/bash.md]
---

# Use the terminal

Phoenix gives each conversation a real shell in the browser, attached to its
[tmux](../reference/tools/tmux.md) session when tmux is available — so your
commands and scrollback survive a Phoenix restart or a closed tab.

## Before you start

You're in a conversation with a working directory.

## Steps

1. **Open the terminal panel.** Expand it from its header (the toggle reads
   **Expand terminal** / **Collapse terminal**). It runs your shell, attached to
   the conversation's tmux `main` session when tmux is present.
2. **Run commands** as you normally would. Because it's tmux-backed, output and
   scrollback persist across restarts and tab close — the same session the agent
   uses via [tmux](../reference/tools/tmux.md).
3. **Turn on shell integration (recommended).** If Phoenix shows **Shell
   integration not detected**, open the snippet (**Enable shell integration**)
   and either **Copy to clipboard** into your shell's rc file, or click **Let
   Phoenix set this up for me** to have it installed for you. With integration
   on, the header HUD shows the running command, its directory, and exit code.
4. **If the shell exits,** the header shows **Shell exited** — click it to start
   a fresh one.

## Result

A persistent, conversation-scoped terminal over your worktree, with a live status
HUD once shell integration is enabled.

## See also

- [tmux](../reference/tools/tmux.md) — why sessions persist
- [Workspace](../concepts/workspace.md) — the tmux server is WorkScope-owned
- [bash](../reference/tools/bash.md) — the agent's non-TTY command tool
