---
title: Modes
summary: The four conversation modes — Direct, Explore, Work, Branch — and how much freedom each gives the agent.
category: concepts
keywords: [direct, explore, work, branch, worktree, mode, read-only]
related: [concepts/tasks.md, concepts/workspace.md, howto/run-a-managed-task.md, reference/modes-matrix.md]
---

# Modes

Every conversation has a **mode**. The mode decides two things: whether the
agent gets an isolated git **worktree** to work in, and which tools it may use —
in particular, whether it can *write* to your files.

## Why it exists

You don't always want the same level of autonomy. Sometimes you want a quick
answer with full access to your checkout. Sometimes you want the agent to
investigate before it's allowed to change anything, and to get your sign-off on
a plan first. Modes make that spectrum explicit and enforce it structurally —
a read-only mode physically cannot write, rather than merely being asked not to.

## How it works

There are four modes:

| Mode | Worktree | Can write? | Use it when… |
|------|----------|-----------|--------------|
| **Direct** | No — your checkout | Yes | You want fast, full access and don't need isolation. Default for non-git folders; **discouraged** for git repos. |
| **Explore** | Temporary, read-only | No (except drafting a task file) | You want the agent to investigate and propose a plan before touching code. |
| **Work** | The task's branch | Yes | A proposed task was approved; the agent executes it on its own branch. |
| **Branch** | An existing branch | Yes | You want to work directly on a branch you name, with no plan-approval step. |

**Managed** conversations are the Explore → Work lifecycle taken together: they
start in Explore, and upgrade to Work only when you approve a task the agent
[proposes](tasks.md). On approval the temporary branch is renamed to the real
task branch, a task file is committed on it, and write tools switch on.

### How you choose one

You don't pick a `ConvMode` directly. For a non-git folder you get Direct, full
stop. For a git repo, the new-conversation screen shows a **Workflow** chooser
that maps onto these modes:

| Workflow card | Becomes |
|---------------|---------|
| **Chat in a fresh worktree** *(recommended)* | Managed → starts in Explore |
| **Start from a task** *(only if the repo has task files)* | Managed → Explore, seeded to propose that task |
| **Chat in a specific branch** | Branch |
| **Work in this folder** *(discouraged)* | Direct |

See [Run a managed task](../howto/run-a-managed-task.md) for the full walkthrough.

Explore is read-only with one deliberate exception: the agent may use `patch`
**inside the `tasks/` directory** so it can draft the task file it's asking you
to approve. Nothing else in your tree can change until you say yes.

Worktrees are isolated git checkouts derived from the conversation's ID, so two
conversations can never collide on the same working directory. See
[Workspace](workspace.md) for how worktrees and the resources inside them
(shells, browser sessions) are owned.

## What you'll see

- A **mode badge** on the conversation header and list rows, reading `Explore`,
  `Work`, `Direct`, or `Branch`. Hovering names the family — Explore and Work
  both read *"Managed mode (…)"*.
- In Explore, write tools are absent and a banner reads *"This is an Explore
  conversation — the agent can read and analyze the codebase but won't make
  changes."*
- When the agent proposes a task, a review surface opens: annotate lines and
  **Send Feedback** to request revisions, **Discard** to reject, or **Approve**
  to start Work.
- In Work and Branch, a **Done?** action bar offers **View Diff**, PR cleanup
  (**Mark as Merged** / **Clean up merged PR**), and **Abandon** — and **PR
  status** is the branch's health indicator once you push.

## See also

- [Tasks](tasks.md) — the propose → approve lifecycle that drives Explore → Work
- [Workspace](workspace.md) — worktrees and resource ownership
- [Run a managed task](../howto/run-a-managed-task.md) — the end-to-end walkthrough
- [Modes matrix](../reference/modes-matrix.md) — exact tool availability per mode
