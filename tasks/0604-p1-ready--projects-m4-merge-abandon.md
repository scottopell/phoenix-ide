---
created: 2026-03-05
number: 604
priority: p1
status: ready
slug: projects-m4-merge-abandon
title: "Projects M4: Merge and abandon flows (complete lifecycle)"
---

# Projects M4: Merge and Abandon

## Summary

Close the lifecycle loop: merge a task branch to main, or abandon and clean up.
Conversation returns to Explore after either path.

## Context

Read first:
- `specs/projects/requirements.md` -- REQ-PROJ-009, REQ-PROJ-010, REQ-PROJ-011
- `specs/bedrock/design.md` -- "Merge Approval State" section
- Task 0603 (M3) -- establishes worktree-based Work mode

## Dependencies

- Task 0603 (M3: worktree isolation)

## What to Do

### Backend

1. **AwaitingMergeApproval state:** New ConvState variant. Entered when agent calls
   `update_task` with status `ready-for-review` and the user approves the status
   update. Holds task_id, diff_summary, and reply channel.

2. **Diff generation:** On entering AwaitingMergeApproval, generate a summary of
   changes between the task branch and main (file list, insertions/deletions).
   Present via SSE event with diff content.

3. **Merge flow:** On user approval:
   - Merge task branch to main (fast-forward or `--no-ff` merge commit)
   - Delete worktree directory (`git worktree remove`)
   - Delete the branch (`git branch -d`)
   - Update task file status to `done` on main via `git commit --only`
   - Remove from worktree registry
   - Transition conversation to Explore mode pinned to new main HEAD

4. **Changes requested:** Return feedback to agent as user message, stay in Work
   mode. Agent continues working in the worktree.

5. **Abandon flow:** On user abandon:
   - Delete worktree directory and branch
   - Update task file status to `abandoned` on main (preserve file as history)
   - Remove from worktree registry
   - Transition to Explore mode
   - Confirm dialog in UI warns that worktree changes are lost

6. **Ambient main advancement (REQ-PROJ-011):** Background watcher monitors main
   branch for new commits. When main advances:
   - Explore conversations: show ambient "N commits behind" indicator
   - Work conversations: notify agent that main has advanced, offer rebase
     opportunity before merge step
   - Neither case interrupts the current conversation

### Frontend

7. **Merge approval UI:** Show diff summary in a review panel. Approve Merge and
   Request Changes actions.

8. **Abandon confirmation:** Confirm dialog warning that worktree changes will be
   lost permanently.

9. **Main advancement indicator:** Subtle badge on Explore conversations showing
   how many commits behind their pinned snapshot is.

10. **Return to Explore:** After merge or abandon, conversation visually
    transitions back to Explore mode (badge update, branch name removed).

## Acceptance Criteria

- [ ] `update_task` with `ready-for-review` (after user approval) enters AwaitingMergeApproval
- [ ] Diff summary generated and shown to user
- [ ] Approve merges branch to main, cleans up worktree and branch
- [ ] Task file updated to `done` on main after merge
- [ ] Request-changes returns feedback to agent, stays in Work mode
- [ ] Abandon deletes worktree and branch, updates task to `abandoned`
- [ ] Conversation returns to Explore mode after merge or abandon
- [ ] Main advancement indicator shows on stale Explore conversations
- [ ] Rebase offered to Work conversations when main advances
- [ ] `./dev.py check` passes

## Value Delivered

Complete lifecycle: Explore -> propose -> approve -> work -> review -> merge ->
Explore. Users can stop after any step and still have a clean, consistent state.
The abandoned task file preserves a record of what was attempted without carrying
any code changes forward.
