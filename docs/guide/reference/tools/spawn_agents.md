---
title: spawn_agents
summary: Spawn parallel sub-agents — up to 10 per call, at most one writing (Work) sub-agent at a time.
category: reference
keywords: [spawn_agents, sub-agent, parallel, explore, work, max_turns]
related: [concepts/sub-agents.md, howto/spawn-sub-agents.md, reference/glossary.md]
---

# spawn_agents

> **At a glance:** the agent delegates independent tasks to child conversations
> that run concurrently and report back. **≤10 per call; ≤1 Work (writing)
> sub-agent active per parent.**

## What it does

Launches [sub-agents](../../concepts/sub-agents.md) — isolated child
conversations — each running its task to completion and submitting one result.

## Per-task parameters

| Field | Default | Notes |
|-------|---------|-------|
| `task` | — | the task description (required) |
| `mode` | `explore` | `explore` (read-only) or `work` (writes) |
| `model` | cheapest (Explore) / parent's (Work) | model override |
| `max_turns` | 20 (Explore) / 50 (Work) | turn budget |
| `cwd` | parent's dir | Work sub-agents must stay inside the worktree |
| `agent_type` | — | a named agent persona |

## What you'll see

The **sub-agent viewer** docks beside the chat with each child's live transcript;
when all finish, their results fold back into the parent.

## Limits & gotchas

- **≤10** sub-agents per call; **20-minute** batch timeout.
- **One writer:** at most one active Work sub-agent per parent; an Explore parent
  can spawn Explore sub-agents only.
- Sub-agents can't spawn sub-agents, ask questions, propose tasks, or load skills.

## Related

- [Sub-agents](../../concepts/sub-agents.md) — the model and guarantees
- [Spawn sub-agents](../../howto/spawn-sub-agents.md) — directing and watching
- [Glossary](../glossary.md) — canonical terms
