---
created: 2026-04-12
priority: p1
status: ready
artifact: pending
---

# auto-mark-task-done-on-squash-merge

## Problem

When Phoenix performs a squash merge from a Work conversation into `main`,
the user is currently shown a blocking dialog:

> **Task file not marked done.**
> Ask agent to fix.
> [Dismiss]

This is a degenerate workflow. The agent already has:
- The task file path
- Write access to the task file
- Authority to commit
- Deterministic knowledge that the merge is happening right now

Asking the user to push a button and hope the agent does the right thing is
worse than just doing the thing. The human is not adding any judgment here —
it's purely a confirmation ritual with no real decision.

## What the system should do instead

WHEN a squash merge is about to run on a Work conversation
AND the task file's frontmatter `status` is not already `done`
THE SYSTEM SHALL update the frontmatter `status` to `done`
AND SHALL rename the file to reflect the new status (per the `NNNN-pX-status--slug.md`
convention enforced by `./dev.py tasks validate`)
AND SHALL include the rename + frontmatter update in the same squash merge
commit (so the task lifecycle is atomic with the merge)

WHEN displaying the merge plan UI
THE SYSTEM SHALL show the exact git command(s) it proposes to run,
including any `git mv` / `git add` steps for the task file update,
AND SHALL require user approval on the whole plan (not on individual
sub-steps)

WHEN the user approves the merge plan
THE SYSTEM SHALL execute it without further prompts about task file state

## Scope notes

- The task-file-update should happen in the same commit as the squash merge,
  not as a follow-up commit. This keeps `git log` and `./dev.py tasks validate`
  consistent in all states.
- If the frontmatter already shows `done`, skip the update (idempotent).
- If the filename already matches the frontmatter, skip the rename (idempotent).
- This is purely a UI/flow change — no new capability for the agent.

## Why p1

It's small but it breaks flow every single merge. Each time it's a
distraction and a friction point in an otherwise-clean workflow. The fix
is well-scoped and the surface area is contained to the merge plan code
path (probably `src/runtime/executor.rs` or wherever `propose_task_complete`
lives) + the frontend merge plan modal.

## Out of scope

- General task auto-management (the agent editing task files outside the
  merge context). That's a bigger design question and not urgent.
- Refactoring the task validator to be more permissive about filename
  drift. The invariant "filename matches frontmatter" is load-bearing for
  `ls tasks/*-ready--*.md` patterns and should stay.
