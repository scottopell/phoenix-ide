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

Close the lifecycle loop: merge worktree to main, or abandon and clean up.
Conversation returns to Explore after either path.

## Context

Read first:
- `specs/projects/requirements.md` — REQ-PROJ-009, REQ-PROJ-010, REQ-PROJ-011
- `specs/bedrock/design.md` — "Merge Approval State" section

## Dependencies

- Task 0603 (M3: worktree isolation)

## What to Do

### Backend

1. **AwaitingMergeApproval state:** New ConvState variant. Entered when agent calls
   `update_task` with `ready-for-review`. Holds task_id, diff_summary, and a
   oneshot reply channel.

2. **Diff generation:** On entering AwaitingMergeApproval, generate a summary of
   changes between the worktree branch and main (file list, insertions/deletions,
   key changes).

3. **Merge flow:** On user approval:
   - Merge worktree branch to main (fast-forward or merge commit)
   - Delete worktree directory
   - Delete the branch
   - Update task status to `done` on main
   - Remove from worktree registry
   - Transition conversation to Explore mode pinned to new main HEAD

4. **Changes requested:** Return feedback to agent, stay in Work mode. Agent
   continues working in the worktree.

5. **Abandon flow:** On user abandon:
   - Delete worktree directory and branch
   - Update task status to `abandoned` on main (preserve file as history)
   - Remove from worktree registry
   - Transition to Explore mode

6. **Ambient main advancement (REQ-PROJ-011):** When main receives new commits
   (from other conversations merging), show an indicator on Explore conversations
   that their pinned snapshot is behind. Offer rebase to Work conversations before
   merge.

### Frontend

7. **Merge approval UI:** Show diff summary in a review panel. Approve and
   request-changes actions.

8. **Abandon confirmation:** Confirm dialog warning that worktree changes are lost.

9. **Main advancement indicator:** Subtle badge on Explore conversations showing
   "N commits behind main."

## Acceptance Criteria

- [ ] `update_task` with `ready-for-review` enters AwaitingMergeApproval
- [ ] Diff summary generated and shown to user
- [ ] Approve merges branch to main, cleans up worktree
- [ ] Task file updated to `done` on main after merge
- [ ] Request-changes returns feedback, stays Work
- [ ] Abandon deletes worktree, updates task to `abandoned`
- [ ] Conversation returns to Explore after merge or abandon
- [ ] Main advancement indicator shows on stale Explore conversations
- [ ] `./dev.py check` passes

## Value Delivered

Complete lifecycle: Explore -> propose -> approve -> work -> review -> merge ->
explore. The natural project rhythm. Users can stop after any step and still have
a clean, consistent state.
