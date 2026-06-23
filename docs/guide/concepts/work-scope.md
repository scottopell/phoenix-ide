---
title: Work scope
summary: The work scope — the container that owns a conversation's live resources (shells, tmux, browser), keyed by worktree or conversation.
category: concepts
keywords: [work scope, workscope, worktree, resources, bash, tmux, browser, continuation, workspace]
related: [concepts/conversations.md, concepts/modes.md, reference/tools/bash.md, reference/glossary.md]
---

# Work scope

A **work scope** is the container for the live resources a conversation spawns —
backgrounded shell commands, a tmux server, a browser session. It's how Phoenix
tracks what's still running and who owns it. (The code calls it `WorkScope`.)

```
 Work scope
   ├─ bash handles    backgrounded commands (capped per scope)
   ├─ tmux server     persistent sessions
   └─ browser session
```

## How it's keyed

A work scope is keyed by **where the work lives**, not by the conversation:

- **Work, Branch, Explore** → keyed by the **worktree** (the git checkout). Every
  conversation on that worktree shares one scope.
- **Direct** → keyed by the **conversation** itself (no worktree to share).

That keying is the point: when you [continue](conversations.md) a conversation
onto the same worktree, the successor **inherits the running resources** — a
build you kicked off in the background, a tmux session, an open browser — instead
of losing them at the boundary. Resources are torn down only when no remaining
conversation shares the scope.

This is also the line between the two ideas: a **worktree** is just the git
checkout; the **work scope** is the resource container *keyed to* it (or, in
Direct, to the conversation).

## What you'll see

A **Work scope** panel (a section of the file-explorer rail on a conversation,
and a dock on a chain) lists the scope's live resources with a running-count
badge and a glyph per resource — running, exited, or failed. It's read-only
observability — what the agent has running, not a control panel.

> **Remember:** resources belong to the **work scope**, not to one conversation.
> The scope spans a worktree, so a continuation inherits the running build, tmux,
> and browser — except in **Direct**, where the scope *is* the conversation.

## See also

- [Conversations](conversations.md) — what spawns the resources
- [Modes](modes.md) — which modes get a worktree
- [bash](../reference/tools/bash.md) — handles, the per-scope cap
- [Glossary](../reference/glossary.md) — work scope, worktree, handle
