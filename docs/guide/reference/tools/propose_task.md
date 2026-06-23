---
title: propose_task
summary: The agent proposes a task for your approval — the Explore→Work gate, or a decoupled fork from a writing mode.
category: reference
keywords: [propose_task, approve, explore, work, fork, task]
related: [concepts/tasks.md, reference/managed-lifecycle-states.md, concepts/modes.md]
---

# propose_task

> **At a glance:** how the agent asks to start real work. In **Explore** it's the
> blocking gate to **Work**; from a writing mode it's a non-blocking **fork**.

## What it does

- **In Explore** — the agent calls `propose_task` with a task file; the
  conversation pauses for your review, and approval renames the branch, commits
  the file, and switches to [Work](../../concepts/modes.md).
- **From a writing mode** (Work / Branch / Direct-in-a-git-repo) — it proposes a
  *fork*: a self-contained task that, on approval, spawns a fresh top-level Work
  conversation off the repository's default branch, leaving the origin running.

## What you'll see

The **task-approval reader** opens — **Approve**, **Send Feedback**, or
**Discard**; every state is in
[Managed lifecycle states](../managed-lifecycle-states.md#task-approval-reader).

## Limits & gotchas

- A taskmd-named file (`NNNNN-pX-…`) must live under `tasks/`, and its status
  must be **`ready`, `in-progress`, or `brainstorming`** — `blocked`, `done`, and
  `wont-do` are rejected.
- Sub-agents can't call it; it's withheld from Direct outside a git repo.
- The two task-file forms produce [different branch names](../../concepts/tasks.md#from-plan-to-branch).

## Related

- [Tasks](../../concepts/tasks.md) — the plan it proposes
- [Managed lifecycle states](../managed-lifecycle-states.md) — the approval states
- [Modes](../../concepts/modes.md) — Explore vs the writing modes
