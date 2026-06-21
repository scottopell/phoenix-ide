---
title: Modes
summary: The four conversation modes — Direct, Explore, Work, Branch — and how much freedom each gives the agent.
category: concepts
keywords: [direct, explore, work, branch, worktree, mode, read-only]
related: [concepts/tasks.md, howto/run-a-managed-task.md, reference/managed-lifecycle-states.md, reference/glossary.md]
---

# Modes

A conversation's **mode** decides how much rope the agent gets: whether it works
in an isolated git **worktree** or your live checkout, and whether it can write
at all. The four modes form a spectrum from safest to most direct:

```
 safest ◀──────────────────────────────────────────────▶ most direct
 Explore ─────▶ Work        Branch            Direct
 read-only      approved    your branch       your checkout
 plan first     writes      writes            writes
```

## The four modes

| Mode | Worktree | Can write? | Use it when… |
|------|----------|-----------|--------------|
| **Direct** | No — your checkout | Yes | You want fast, full access and don't need isolation. Default off-git; **discouraged** for git repos. |
| **Explore** | Temporary, read-only | No (except a task file) | You want the agent to investigate and propose a plan before touching code. |
| **Work** | The task's branch | Yes | A proposed task was approved; the agent executes it on its own branch. |
| **Branch** | An existing branch | Yes | You want to edit a branch you name, with no plan-approval step. |

## The managed lifecycle

Explore and Work are two halves of one **managed** lifecycle. A managed
conversation starts read-only in Explore; when you approve a task the agent
[proposes](tasks.md), Phoenix renames the temporary branch, commits the task
file on it, and unlocks writes as **Work**. (Explore's lone write exception: the agent may
`patch` inside `tasks/` to draft that very plan.)

You never select a mode directly. For a git repo you pick a **Workflow** card on
the new-conversation screen and Phoenix derives the mode — see
[Run a managed task](../howto/run-a-managed-task.md). Worktrees are isolated
checkouts keyed to the conversation, so two conversations never collide and your
working copy stays untouched.

## What you'll see

A **mode badge** on the conversation header and list rows shows the current
mode, and in Explore the write tools are simply absent. The per-mode controls
are covered in the [walkthrough](../howto/run-a-managed-task.md).

> **Remember:** a read-only mode *cannot* write — that's enforced, not asked.
> And a worktree is a separate checkout, so your working copy is never touched
> until you approve.

## See also

- [Conversations](conversations.md) — what a mode is a property of
- [Tasks](tasks.md) — the plan that gates Explore → Work
- [Workspace](workspace.md) — the worktree a mode runs in
- [Run a managed task](../howto/run-a-managed-task.md) — the end-to-end walkthrough
- [Managed lifecycle states](../reference/managed-lifecycle-states.md) — every approval & Done? state
- [Glossary](../reference/glossary.md) — canonical terms
