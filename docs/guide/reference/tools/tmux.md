---
title: tmux
summary: A persistent, per-workspace tmux server for long-running or interactive processes that survive Phoenix restarts.
category: reference
keywords: [tmux, persistent, terminal, session, tmux_run, restart]
related: [reference/tools/bash.md, howto/use-the-terminal.md, concepts/workspace.md]
---

# tmux

> **At a glance:** the persistence answer to [bash](bash.md)'s ephemerality. A
> tmux server, owned by the [workspace](../../concepts/workspace.md), runs
> long-lived or interactive processes that **survive Phoenix restarts**, tab
> close, and window blur.

## What it does

The tmux server is keyed to the workspace, like the agent's other live resources:
a Work/Branch worktree shares one server across its continuation members, and a
Direct conversation gets its own. Processes the agent starts there keep running
across restarts; the in-app terminal auto-attaches to the `main` session so you
can watch and type.

## Operations

| Tool | Does |
|------|------|
| `tmux` | pass-through tmux subcommands (full CLI: windows, send-keys, capture-pane, …) |
| `tmux_run` | run a command in a window, optionally waiting for a readiness signal |

## What you'll see

The in-app **terminal** attached to the workspace's `main` session — live output
and scrollback for whatever the agent started.

## Limits & gotchas

- Captured output in a tool response is middle-truncated at **128 KB**; the full
  scrollback stays in tmux (read it via `capture-pane` or the terminal).
- Default wait **30 s** (max 900). One in-app attachment at a time.
- Killed only on **hard-delete / archive** — soft state (blur, close tab) never
  kills it. Use tmux when a process must outlive the tab.

## Related

- [bash](bash.md) — ephemeral, no TTY; tmux is the durable counterpart
- [Use the terminal](../../howto/use-the-terminal.md) — attaching and driving it
- [Workspace](../../concepts/workspace.md) — the tmux server is workspace-owned
