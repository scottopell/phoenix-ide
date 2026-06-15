---
title: Run a managed task
summary: Take a change from read-only investigation to a pushed task branch using the Explore → approve → Work lifecycle.
category: howto
keywords: [managed, explore, work, propose_task, approve, task, branch, pr, worktree]
related: [concepts/modes.md, concepts/tasks.md, howto/review-changes.md]
---

# Run a managed task

A *managed* conversation lets the agent investigate your repository read-only,
draft a plan you approve, and only then change code — on its own branch, in an
isolated worktree, leaving your checkout untouched. Use it when a change is big
enough that you want to see the plan before any edits happen.

There is no mode called "Managed" in the UI. You enter the managed lifecycle by
choosing a **Workflow** card on the new-conversation screen; the conversation
then moves through the **Explore** and **Work** modes on its own.

## Before you start

- Point the conversation at a **git repository**. The **Workflow** chooser only
  appears once Phoenix confirms the working directory is a git repo — a non-git
  folder gives you Direct mode only.

## Steps

1. **Set the working directory** to your git repo on the new-conversation
   screen. When it resolves as a git repo, a **Workflow** section appears,
   headed *"Choose how Phoenix should use this Git repository."*
2. **Choose "Chat in a fresh worktree."** This is the recommended hero card
   — *"Start from the default branch in an isolated worktree."* It creates a
   managed conversation: the agent works in an isolated worktree, not your
   checkout. (Don't pick **"Work in this folder"** for this — it's marked
   discouraged and edits your current checkout directly.)
   - *Alternative:* if the repo already has task files, **"Start from a task"**
     appears. Pick a **Base branch for planning** and a **Task file**, and
     Phoenix pre-fills your first message so the agent goes straight to proposing
     that existing task instead of drafting a new one.
3. **Write your request and press Send.** The button reads *"Creating…"* while
   the worktree is set up. The conversation opens in **Explore** — the header
   and list badge read **`Explore`** (tooltip: *"Managed mode (read-only
   exploration)"*).
4. **Let the agent investigate.** A banner explains the rules: *"This is an
   Explore conversation — the agent can read and analyze the codebase but won't
   make changes. When you're ready to modify code, describe what you want and
   the agent will propose a plan for your review."* It can read, search, and run
   read-only commands, and it drafts a task file under `tasks/` — that file *is*
   the plan. Nothing else in your tree can change yet.
5. **Review the proposed plan.** When the agent calls `propose_task`, a review
   surface opens on the task file. To respond:
   - **Annotate** — add a note on any line (*"Add your note…"*), then
     **Send Feedback (N)** to send your notes back for revision; the agent
     refines and proposes again.
   - **Discard** — reject it (*"Discard this task? The agent will be informed
     the task was rejected."*).
   - **Approve** — accept the plan as-is.
6. **Approval starts Work.** Phoenix renames the temporary branch to the real
   task branch, promotes the task file to `in-progress` and commits it on that
   branch, and turns on write tools. The badge flips to **`Work`** (tooltip:
   *"Managed mode (task branch)"*).
7. **Let it execute, and review as it goes.** The agent edits on the task
   branch. In the **Done?** action bar, **View Diff** shows the changes as they
   land — see [Review changes](review-changes.md).
8. **Finish.** When the work is done the agent pushes the branch with a normal
   `git push` (there is no push button — push is just a command the agent runs).
   Open a pull request and merge it on your host; Phoenix never merges for you.
   It tracks PR state, so once the PR is merged the **Done?** bar offers
   **Clean up merged PR** (it reads **Mark as Merged** until then).

## Result

A dedicated task branch pushed to `origin`, carrying the committed task file and
the agent's changes, ready for a PR. Your original checkout was never touched,
and the conversation shows **PR status** as the branch's health indicator.

## Troubleshooting

- **No Workflow section appears.** The directory isn't a git repo, or branch
  metadata is still loading. Managed mode requires git.
- **"Start from a task" is missing.** That card only shows when the repo already
  has active task files. Use **"Chat in a fresh worktree"** and let the agent
  draft one.
- **Send is disabled.** You need message content *and* a resolved starting
  branch; if you chose **"Start from a task"** you must also pick a **Task file**.
  A starting point that already has an active conversation is also blocked.
- **The agent won't edit my code.** It's in Explore (read-only) — approve the
  plan to enter Work.
- **I don't want the plan-approval step.** Use **"Chat in a specific branch"**
  (Branch mode) instead — see [Modes](../concepts/modes.md).
- **I want to drop the work.** Use **Abandon** in the **Done?** bar (*"Abandon
  this task? The worktree and task branch will be deleted."*).

## See also

- [Modes](../concepts/modes.md) — what Explore and Work can and can't do
- [Tasks](../concepts/tasks.md) — the task file as a living contract
- [Review changes](review-changes.md) — diff and prose viewers
