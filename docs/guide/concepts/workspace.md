---
title: Workspace
summary: The WorkScope — the container that owns a conversation's live resources (shells, tmux, browser) and survives continuation.
category: concepts
keywords: [workspace, workscope, worktree, resources, bash, tmux, browser, continuation]
related: [concepts/conversations.md, concepts/modes.md, reference/tools/bash.md, reference/glossary.md]
---

# Workspace

A **WorkScope** is the container for the live resources a conversation spawns —
backgrounded shell commands, a tmux server, a browser session. It's how Phoenix
tracks what's still running and who owns it.

```
 WorkScope
   ├─ bash handles    backgrounded commands (capped per scope)
   ├─ tmux server     persistent sessions
   └─ browser session
```

## How it's keyed

The scope is keyed by **where the work lives**, not by the conversation:

- **Work, Branch, Explore** → keyed by the **worktree**. Every conversation on
  that worktree shares one scope.
- **Direct** → keyed by the **conversation** itself (no worktree to share).

That keying is the whole point: when you [continue](conversations.md) a
conversation onto the same worktree, the successor **inherits the running
resources** — a build you kicked off in the background, a tmux session, an open
browser — instead of losing them at the boundary. Resources are torn down only
when no remaining conversation shares the scope.

## What you'll see

A **work-scope panel** (a section of the file-explorer rail on a conversation,
and a dock on a chain) lists the scope's live resources with a running-count
badge and a glyph per resource — running, exited, or failed. It's read-only
observability: it shows you what the agent has running, it isn't a control panel.

> **Remember:** resources belong to the **worktree**, not the conversation. A
> continuation on the same branch inherits them; teardown waits until no
> conversation shares the scope.

## See also

- [Conversations](conversations.md) — what spawns the resources
- [Modes](modes.md) — which modes get a worktree
- [bash](../reference/tools/bash.md) — handles, the per-scope cap
- [Glossary](../reference/glossary.md) — WorkScope, worktree, handle
