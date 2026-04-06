---
created: 2026-04-03
priority: p1
status: done
artifact: src/runtime/executor.rs
---

# Fix non-atomic task approval (commit before worktree)

## Summary

`execute_approve_task_blocking` commits the task file to main (step 5-6) before
creating the worktree (step 8). If worktree creation fails (disk full, path
issue, permission error), the executor returns Err and retries -- but the commit
is already on main. Retries create new task IDs and new commits, orphaning the
previous ones.

## What to change

Reorder the git sequence: create the worktree first, then commit the task file.
Or: defer the commit until after worktree creation succeeds. If worktree
creation fails, no artifacts are left on main.

Alternative: if the commit must come first (branch creation needs it), add a
rollback step: `git reset --soft HEAD~1` on worktree creation failure.

## Done when

- [ ] Failed worktree creation does not leave orphaned commits on main
- [ ] Successful approval still produces: task file commit, worktree, branch
- [ ] Test or manual verification of failure-then-retry path
