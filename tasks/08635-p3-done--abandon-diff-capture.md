---
created: 2026-04-07
priority: p3
status: done
artifact: src/api/handlers.rs
---

# Capture git diff as message before abandon destroys worktree

## Summary

When abandoning a conversation, the worktree and branch are deleted permanently.
Any uncommitted or unmerged work is lost with no trace. This raises the psychological
cost of abandoning -- users hesitate because they might lose an interesting idea.

Capture the diff before deletion and record it as a system message in the
conversation, so the work is still visible in the chat history even after
the worktree is gone.

## What to do

In the abandon handler, before deleting the worktree:

1. Run `git diff HEAD` in the worktree to capture uncommitted changes
2. Run `git log --oneline {base_branch}..HEAD` to capture committed-but-unmerged work
3. If combined output is under ~100KiB, record it as a system message (or user
   message with `is_meta` flag) containing the diff in a code fence
4. If over 100KiB, record a summary message noting the diff was too large, with
   the line/file count

The message should be recorded before the worktree is deleted so it persists
in the conversation history regardless of whether the abandon succeeds.

## Done when

- [ ] Abandon captures uncommitted diff as a conversation message
- [ ] Abandon captures unmerged commits log as a conversation message
- [ ] Large diffs are truncated with a summary instead of omitted
- [ ] Message is visible in the conversation after abandon completes
