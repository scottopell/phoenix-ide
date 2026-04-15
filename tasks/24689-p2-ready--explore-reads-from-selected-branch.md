---
created: 2026-04-15
priority: p2
status: ready
artifact: src/runtime/executor.rs
---

# Explore mode should read from the selected branch, not main checkout

## Summary

When a branch is selected in the branch picker, the Explore phase should
read from that branch's state, not the main checkout. Currently the agent
explores whatever the main checkout happens to be (which may be dirty,
detached HEAD, or on a completely different branch), then the worktree is
created off the selected branch at approval time. The exploration is
reading the wrong code.

## Options

1. **Create a read-only worktree for Explore** -- same git ops as Work mode
   but no task file, no commit. Agent reads from the branch's actual state.
   Cleanup on conversation end or on approval (upgrade to Work worktree).

2. **Use `git show <branch>:<path>` for file reads** -- no worktree, but
   file reads are redirected to the selected branch. More complex, breaks
   tools that shell out (bash, grep).

3. **Skip Explore entirely** -- see task 24687. If the user knows the branch,
   the Explore phase may be unnecessary overhead.

## Context

Filed during QA. The root checkout was in detached HEAD state while the user
wanted to work on q-branch-observer. The Explore agent reported the detached
HEAD state instead of the branch's state.
