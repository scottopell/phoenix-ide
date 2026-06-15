---
title: Run a managed task
summary: Take a change from read-only investigation to a pushed task branch using the Explore → approve → Work lifecycle.
category: howto
keywords: [managed, explore, work, propose_task, approve, task, branch, pr, worktree]
related: [concepts/modes.md, reference/managed-lifecycle-states.md, reference/glossary.md]
---

# Run a managed task

Let the agent investigate read-only, draft a plan you approve, and only then
change code — on its own branch in an isolated worktree, leaving your checkout
untouched. You enter this lifecycle by choosing a **Workflow** card; the
conversation moves through [Explore and Work](../concepts/modes.md) on its own.

## Before you start

Point the conversation at a **git repository** — the **Workflow** chooser only
appears once the working directory resolves as one.

## Steps

1. **Set the working directory** to your git repo. A **Workflow** section
   appears, headed *"Choose how Phoenix should use this Git repository."*
2. **Pick "Chat in a fresh worktree"** — *"Recommended for Git repos. Start from
   the default branch in an isolated worktree."* It uses the repo's default
   branch, so there's no branch picker. (Don't pick **"Work in this folder"** —
   it edits your checkout directly.)
   - *From an existing task instead:* **"Start from a task"** appears when the
     repo has active task files (any status but `done`/`wont-do`). Pick a **Base
     branch for planning** and a **Task file**; your first message is seeded to
     propose that task.
3. **Write your request and press Send.** The conversation opens in **Explore**
   (badge `Explore`); the agent investigates read-only and drafts a task file
   under `tasks/` — that file is the plan.
4. **Review the plan.** When the agent calls `propose_task`, a review surface
   opens. **Approve** it, annotate lines and **Send Feedback (N)** to revise, or
   **Discard** to reject. The surface has no dismiss — see every state in
   [Managed lifecycle states](../reference/managed-lifecycle-states.md#task-approval-reader).
5. **Work begins on approval.** Phoenix renames the temporary branch to the task
   branch, commits the task file on it, and enables writes — the badge flips to
   `Work`. Review changes as they land with **View Diff** in the **Done?** bar.
6. **Finish.** The agent pushes the branch with `git` (there's no push button).
   Open a PR and merge it on your host; the **Done?** bar then offers **Clean up
   merged PR**. To iterate on PR feedback use **Address CI & comments**; to drop
   the work, **Abandon**. The bar's full state set — six completion labels, the
   `gh` fallback, the continuation lock — is in
   [Managed lifecycle states](../reference/managed-lifecycle-states.md#the-done-bar).

## Result

A task branch pushed to `origin`, carrying the committed task file and the
agent's changes, ready for a PR. Your checkout was never touched, and **PR
status** tracks the branch's health.

## Troubleshooting

- **No Workflow section.** The directory isn't a git repo, or branch metadata is
  still loading.
- **"Start from a task" is missing.** The repo has no active task files (all are
  `done`/`wont-do`). Use **"Chat in a fresh worktree"** and let the agent draft one.
- **Send is disabled.** You need message content and a resolved starting point;
  **"Start from a task"** also requires a chosen **Task file**. A starting point
  that already has an active conversation is blocked.
- **The agent won't edit my code.** It's in Explore (read-only) — approve the plan.
- **The Done? bar isn't there.** It shows only in `Work`/`Branch` while idle,
  errored, or out of context — not while the agent is running.
- **I want to skip the plan step.** Use **"Chat in a specific branch"** (Branch
  mode) — see [Modes](../concepts/modes.md).

## See also

- [Modes](../concepts/modes.md) — what Explore and Work can and can't do
- [Managed lifecycle states](../reference/managed-lifecycle-states.md) — every state of the approval and Done? surfaces
- [Glossary](../reference/glossary.md) — canonical terms
