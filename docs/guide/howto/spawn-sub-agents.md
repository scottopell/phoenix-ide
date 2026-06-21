---
title: Spawn sub-agents
summary: Get the agent to fan work out across parallel sub-agents, and watch them as they run.
category: howto
keywords: [sub-agent, spawn, parallel, explore, work, viewer]
related: [concepts/sub-agents.md, concepts/conversations.md, reference/glossary.md]
---

# Spawn sub-agents

[Sub-agents](../concepts/sub-agents.md) let one conversation fan work out in
parallel. You don't spawn them directly — you ask for work that parallelizes, and
the agent spawns and supervises them; your job is to watch and read the results.

## Before you start

For parallel **reading/investigation**, any conversation works. For a sub-agent
that **writes**, the parent must be in a writing mode (Work, Branch, or Direct) —
an Explore parent can only spawn read-only Explore sub-agents.

## Steps

1. **Ask for parallelizable work.** Describe a task with independent parts —
   "investigate these three modules in parallel", or "explore X while you handle
   Y". The agent decides when to spawn sub-agents.
2. **Watch them.** When the agent opens a sub-agent, a viewer docks beside the
   chat with that child's transcript, live while it runs and marked read-only.
3. **Open or switch.** Open a sub-agent to read its full transcript; a link opens
   it as a full page if you want room.
4. **Let them report back.** Each sub-agent submits one result; the parent reads
   them in and continues the main thread.

## Result

Parallel investigation or work, folded back into the parent conversation without
you managing each child.

## Troubleshooting

- **Only one thing is writing at a time.** By design: read-only Explore
  sub-agents fan out freely, but only **one** Work (writing) sub-agent runs per
  parent at a time, inside the parent's worktree (see
  [Sub-agents](../concepts/sub-agents.md)).
- **The agent won't spawn a writer from Explore.** An Explore parent is read-only
  — approve a task to reach Work first.

## See also

- [Sub-agents](../concepts/sub-agents.md) — the model and its guarantees
- [Conversations](../concepts/conversations.md) — a sub-agent is one
- [Glossary](../reference/glossary.md) — canonical terms
