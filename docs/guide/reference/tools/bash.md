---
title: bash
summary: Runs shell commands as background-capable children of Phoenix, captured into a scrollback ring — no terminal attached.
category: reference
keywords: [bash, shell, command, handle, still_running, kill, peek, tmux]
related: [concepts/modes.md, reference/glossary.md]
---

# bash

> **At a glance:** the agent runs a shell command; if it doesn't finish within
> the agent's chosen wait window, it keeps running in the background as a
> **handle**. Max **8 live handles** per workspace. No TTY — interactive
> programs need **tmux** or the in-app **terminal**.

## What it does

Executes a command via `bash -c` as a pipe-backed child of Phoenix, with stdout
and stderr combined into a per-command **ring buffer**. The agent says how long
it's willing to block (`wait_seconds`). If the command exits in time, you see
the exit code and final output. If not, the command becomes a background
**handle** the agent can check on later. A non-zero exit code is a normal
result, not an error.

## Operations

The agent picks one operation per call via `op`:

| `op` | Meaning | What you see |
|------|---------|--------------|
| `run` | Start a command, block up to `wait_seconds` | Exit + output, **or** a `still_running` handle |
| `peek` | Snapshot a handle's current output | Latest ring-buffer lines |
| `wait` | Block again on an existing handle | Same handle on re-timeout — no new handle |
| `kill` | Signal the handle once (no auto-escalation) | Terminal status, or `kill_pending_kernel` if stuck |

Common fields: `cmd` (the shell text), `wait_seconds`, and an optional `label`
that is echoed on every later response carrying that handle.

## Statuses

| Status | Means |
|--------|-------|
| (exit + code) | Finished within the wait window |
| `still_running` | Wait window elapsed; the command runs on as a handle |
| `tombstoned` | The handle finished; final output preserved as a tail |
| `kill_pending_kernel` | Kill signal sent but the process is stuck (e.g. uninterruptible I/O); a late exit still resolves it |

## What you'll see

In the transcript, a bash call shows the command and its captured output. A
long-running command shows a handle label and a `still_running` marker; you can
follow live output (and resource usage) in the **process inspector** panel.

## Limits & gotchas

- **8 live handles per workspace.** At the cap, a new `run` is refused with a
  list of existing handles — nothing is silently evicted. The agent must
  `kill` or `wait` one out first.
- **Ephemeral by design.** Handles do **not** survive a Phoenix or system
  restart. For anything that must persist, the agent uses **tmux**.
- **No TTY.** Programs needing a terminal (pagers, `vim`, prompts) won't behave;
  use **tmux** or the in-app **terminal**.
- **Read-only in Explore.** In [Explore mode](../../concepts/modes.md), bash is
  sandboxed and cannot write your tree.
- **Dangerous commands are gated.** Blind `git add`, force-pushes, and dangerous
  `rm` are screened by the [permission layer](../../concepts/permissions.md) before bash runs.

## Related

- [Workspace](../../concepts/workspace.md) — what "per workspace" means for the handle cap
- [Modes](../../concepts/modes.md) — why bash is read-only in Explore
- [Glossary](../glossary.md) — canonical terms (handle, WorkScope)
