---
title: Run a managed task
summary: Take a change from investigation to a pushed branch using the Explore → approve → Work lifecycle.
category: howto
keywords: [managed, explore, work, propose_task, approve, task, branch, pr]
related: [concepts/modes.md, concepts/tasks.md, howto/review-changes.md]
---

# Run a managed task

Managed mode lets the agent investigate your repository read-only, propose a
plan you approve, and only then make changes — on its own branch, leaving your
checkout untouched. Use it when a change is big enough that you want a plan
before any edits happen.

## Before you start

- Open a **git repository** as a project — managed mode requires git.
- Skim [Modes](../concepts/modes.md) and [Tasks](../concepts/tasks.md) so the
  vocabulary below is familiar.

## Steps

1. **Start a managed conversation.** In the mode picker for a new conversation,
   choose **Managed**. The conversation opens in **Explore** mode.
2. **Describe the work.** Send your first message. Phoenix creates a temporary,
   read-only worktree (on a `task-pending-…` branch) and the agent begins
   investigating. It can read and search everything but cannot edit your code.
3. **Let it draft a plan.** As it explores, the agent drafts a task file under
   the project's `tasks/` directory — the one place it's allowed to write in
   Explore. This file *is* the plan.
4. **Review the proposal.** When the agent calls `propose_task`, Explore pauses
   and you get a review surface. Read the plan and either:
   - **Approve** — accept it as-is;
   - **Request changes / annotate** — send it back for revision; the agent
     refines and proposes again.
5. **Approval upgrades to Work.** On approval, the temporary branch is renamed
   to `task-{NNNN}-{slug}`, the task file is promoted to `in-progress` and
   committed on that branch, and write tools turn on. The conversation is now in
   **Work** mode.
6. **Let it execute, and watch.** The agent edits code on the task branch. Use
   the diff and prose viewers to [review changes](review-changes.md) as they
   land.
7. **Push for review.** When the work is done the agent pushes the branch to
   `origin` with a normal `git push`. You merge it through a pull request on
   your hosting platform — Phoenix never merges for you.

## Result

A dedicated task branch pushed to `origin`, carrying the committed task file and
the agent's changes, ready for a PR. Your original checkout was never modified.
Phoenix shows **PR status** as the branch's health indicator, and offers a
cleanup affordance once the PR is merged.

## Troubleshooting

- **The agent won't edit my code in Explore.** That's by design — Explore is
  read-only. Approve a task to enter Work mode.
- **I picked the wrong plan.** Use *Request changes* on the proposal rather than
  approving; the agent revises in the same worktree.
- **I want to work on an existing branch with no plan step.** Use **Branch**
  mode instead of Managed — see [Modes](../concepts/modes.md).
- **I'm done but don't want to keep the branch.** Use **Abandon**; in managed
  mode it removes the worktree and the task branch (a diff snapshot is captured
  first).

## See also

- [Modes](../concepts/modes.md) — what Explore and Work can and can't do
- [Tasks](../concepts/tasks.md) — the task file as a living contract
- [Review changes](review-changes.md) — diff and prose viewers
