---
title: Tasks
summary: The task file — the plan the agent drafts and you approve — and how approval turns it into a working branch.
category: concepts
keywords: [task, propose, approve, task file, task branch, taskmd, status, priority]
related: [concepts/modes.md, howto/run-a-managed-task.md, reference/glossary.md]
---

# Tasks

A **task** is the unit of agreed work: a plan the agent drafts and you approve
*before* it changes code. It lives as a versioned file in the repo's `tasks/`
directory — a living contract, not a throwaway prompt. The filename *is* the
metadata:

```
tasks/24691-p1-in-progress--fix-login.md
      └─id─┘ │  └──status──┘   └─slug─┘
           priority
```

- **id** — five digits, allocated once.
- **priority** — `p0` (critical) … `p4` (nice-to-have).
- **status** — `ready`, `in-progress`, `blocked`, `brainstorming`, `done`, `wont-do`.

The body is free-form markdown; the filename carries the metadata, so changing
status is just a rename.

## From plan to branch

In a [managed](modes.md) conversation the agent drafts a task file in Explore and
calls `propose_task`; your approval is what unlocks Work. On approval the
temporary `task-pending-{id}` branch is renamed and the file is committed on it —
the exact branch name depends on the plan's form:

| Plan form | Task branch | On approval |
|-----------|-------------|-------------|
| taskmd file (`NNNNN-pX-…`) | `task-{id}-{slug}` (full 5-digit id) | status promoted to `in-progress` |
| plain `.md` file | `task-{sanitized-stem}-{conv-id-prefix}` | committed as-is, no status change |

## What you'll see

A collapsible **Tasks** panel in the conversation lists the repo's tasks, grouped
active vs. closed and flagging the one this conversation is working on. Open a
task to read its metadata and body, start a conversation on it, or jump to the
one already working it.

> **Remember:** the task file is the contract. Approving it is what turns
> read-only Explore into write-enabled Work — no approved task, no writes.

## See also

- [Modes](modes.md) — the Explore → Work lifecycle a task gates
- [Run a managed task](../howto/run-a-managed-task.md) — proposing and approving in practice
- [Glossary](../reference/glossary.md) — task file, task branch
