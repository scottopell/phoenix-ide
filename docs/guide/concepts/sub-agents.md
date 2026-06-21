---
title: Sub-agents
summary: Child conversations the agent spawns to run tasks in parallel, in isolation, reporting results back.
category: concepts
keywords: [sub-agent, spawn, parallel, explore, work, isolation, one-writer, result]
related: [concepts/conversations.md, concepts/modes.md, howto/spawn-sub-agents.md, reference/glossary.md]
---

# Sub-agents

A **sub-agent** is a child [conversation](conversations.md) the agent spawns to
run a task on its own, in parallel with siblings, then report a result back. It's
how one conversation fans out work — exploring several leads at once, or handing
a contained job to a focused worker.

```
 parent ──┬──▶ sub-agent (explore)  ──▶ result ──┐
          ├──▶ sub-agent (explore)  ──▶ result ──┼──▶ parent reads them
          └──▶ sub-agent (work)     ──▶ result ──┘
```

## How they work

- **Isolated.** Each runs as its own conversation with a stripped toolset — a
  sub-agent can't spawn its own sub-agents, ask you questions, or propose tasks.
  An **Explore** sub-agent is read-only; a **Work** sub-agent can write.
- **Bounded.** Each has a turn budget and a wall-clock timeout, and a parent can
  fan out several at once. Explore sub-agents are read-only investigators; the
  costly write path is held to one at a time.
- **One writer.** At most **one Work sub-agent runs per parent at a time**, and
  when the parent has a worktree (Work/Branch) the Work sub-agent stays inside it.
  Concurrent writers per parent can't happen — it's rejected at spawn.
- **Reports back.** A sub-agent ends by submitting exactly one result (or an
  error); the parent reads those in and continues.

## What you'll see

When the agent opens a sub-agent, a **sub-agent viewer** docks beside the chat
with that child's full transcript, live while it runs. You watch and read; you
don't drive it — a running sub-agent is parent-driven, a finished one is done.

> **Remember:** only **one** Work (writing) sub-agent runs per parent at a time
> (inside the parent's worktree when it has one). Read-only Explore sub-agents
> fan out freely; concurrent writers are structurally impossible.

## See also

- [Conversations](conversations.md) — a sub-agent is one
- [Spawn sub-agents](../howto/spawn-sub-agents.md) — directing and watching them
- [Modes](modes.md) — Explore (read-only) vs Work (writing)
- [Glossary](../reference/glossary.md) — sub-agent
