---
created: 2026-04-07
priority: p2
status: done
artifact: src/runtime/executor.rs
---

# Clean up worktree on task approval failure

## Problem

`execute_approve_task` creates a worktree early in the approval flow. If a
later step fails (e.g., `git commit` for the task file -- previously due to
SSH agent signing), the worktree and branch are left behind. On retry, the
same conversation ID maps to the same worktree directory path, so
`git worktree add` fails with "already exists."

The user must manually `git worktree remove` + `git branch -D` before
retrying. Hit in production on 2026-04-08 (task-02637 on sopell3).

## Root cause

The approval flow is not transactional. Steps run sequentially:
1. Create branch
2. Create worktree
3. Write task file
4. Git add + commit task file
5. Update conversation mode in DB

If step 4 fails, steps 1-3 have already succeeded and are not rolled back.

## Fix

Wrap the approval flow in a cleanup-on-error pattern. If any step after
worktree creation fails:
- `git worktree remove --force` the directory
- `git branch -D` the branch
- Return the original error

This ensures retry from `awaiting_approval` starts clean.

## Done when

- [ ] Failed task approval cleans up worktree and branch
- [ ] Retry after failure succeeds without manual intervention
