---
title: Overview
summary: The big picture — how Phoenix's pieces fit, and where to start.
category: concepts
keywords: [overview, map, big picture, orientation]
related: [concepts/conversations.md, concepts/modes.md, howto/getting-started.md]
---

# Overview

Phoenix is an LLM coding agent you direct through **conversations**. You describe
what you want; the agent reads and edits your code through **tools**, bounded by
the **mode** the conversation runs in. Everything else hangs off that.

```
 you ─▶ conversation ──▶ agent ──▶ tools (bash, patch, browser, …)
            │  in a mode: Direct / Explore / Work / Branch
            │  on a worktree — the workspace
            ├─ propose ▶ task        (a plan you approve)
            └─ continue ▶ chain      (a run of conversations)
```

## The pieces

- **[Conversation](conversations.md)** — the unit of work: a thread with the
  agent, in a directory, with a mode and a state.
- **[Mode](modes.md)** — how much freedom the agent has, from read-only Explore
  to full-access Direct.
- **[Workspace](workspace.md)** — the worktree and the live resources (shells,
  tmux, browser) a conversation owns.
- **[Task](tasks.md)** — a plan the agent proposes and you approve before it
  writes.
- **[Sub-agents](sub-agents.md)** — child conversations that fan work out in
  parallel.
- **[Skills](skills.md)** — reusable instruction sets you invoke by name.
- **[Chains](chains.md)** — runs of continued conversations, queryable as one.
- **[Permissions](permissions.md)** — the deny layer that blocks unsafe tool
  calls by construction.

## Where to start

New here? [Getting started](../howto/getting-started.md) takes you from an empty
screen to a running conversation.

> **Remember:** everything in Phoenix hangs off a **conversation** — mode,
> workspace, tools, and state are all properties of one.

## See also

- [Conversations](conversations.md) — the root noun
- [Modes](modes.md) — the freedom dial
- [Getting started](../howto/getting-started.md) — your first conversation
