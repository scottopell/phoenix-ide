---
created: 2026-04-08
priority: p0
status: done
artifact: pending
---

# Duplicate git commit block in task approval causes approval to always fail

## Summary

`execute_approve_task_blocking()` in `src/runtime/executor.rs` has a duplicate
`git commit` block (steps 6 and 8). Step 6 commits the task file successfully.
Step 8 tries to commit again with nothing staged, which fails with exit code 1
("nothing to commit"). The error handler then removes the task file from disk
and returns Err, causing task approval to fail.

## Reproduction

1. Start a Managed conversation on a git repo
2. Get the agent to propose a task
3. Approve the task
4. Approval fails at step 8 with "Failed to commit task file"

## Root Cause

Commit `0dd52cd` ("feat: add project management state machine and bedrock spec")
refactored the approval sequence. Previously:
- Step 6 staged only (no commit)
- Step 7 checked branch collision
- Step 8 committed then created branch + worktree

The refactor moved the commit into step 6 (before collision check) but left the
old step 8 commit block in place. Result: two identical `git commit -m` blocks.

## Fix

Delete the duplicate commit block at lines 1970-1978 (step 8). Step 6 already
commits. Step 9 should follow directly after step 7 (branch collision check).

## Lines

- Step 6 (correct): `src/runtime/executor.rs:1938-1948`
- Step 8 (duplicate, remove): `src/runtime/executor.rs:1967-1978`
