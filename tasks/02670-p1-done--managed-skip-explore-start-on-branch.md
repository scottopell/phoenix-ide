---
created: 2026-04-15
priority: p1
status: done
artifact: src/runtime/executor.rs
---

# Managed mode: option to skip Explore and start directly on a branch

## Summary

When the user selects a branch in the branch picker and chooses Managed mode,
provide a way to skip the Explore phase and go directly to Work mode on that
branch. The current Explore -> propose_plan -> approve -> Work flow has a
fundamental disconnect: the agent explores the main checkout (which may be
dirty, detached, or on a different branch) but then works in a worktree off
the selected branch. The exploration is reading the wrong code.

## Real-World Trigger

User has a branch (`q-branch-observer`) with an open PR that needs CI fixes.
The main checkout is in detached HEAD state at a different commit. Managed mode
forces the agent to explore the detached HEAD state first, then proposes a plan
based on that exploration, then creates a worktree off the actual branch. The
exploration is wasted and potentially misleading.

## Proposed Behavior

When the user sends their first message with a branch selected:
- Create the worktree immediately (same git ops as task approval)
- Agent starts in Work mode in the worktree
- No Explore phase, no propose_plan interception
- The agent reads the correct code from the start

This could be a toggle ("Start on branch" vs "Explore first") or the default
when a non-default branch is selected. The Explore -> Work flow still has value
when the user wants the agent to investigate before committing to a plan.

## Context

Filed during QA of branch picker (REQ-PROJ-020-023). The branch picker makes
it easy to select the right branch, but the Explore phase then ignores it.
